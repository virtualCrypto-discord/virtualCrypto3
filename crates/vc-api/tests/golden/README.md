# v2 golden responses (captured from the Elixir implementation)

These files pin the behaviour of the `/api/v2` endpoints, including the two that
have **no test coverage** in the Elixir suite, so the Rust rewrite has an
executable contract:

- `GET /api/v2/users/@me`
- `GET /api/v2/users/@me/balances`

Each file records the status, the response headers and the parsed JSON body for a
fixed fixture. They were produced by running the Elixir application in `test`
mode against the Ecto-migrated database and dispatching requests through
`Phoenix.ConnTest` (see `scripts/capture-v2-golden.exs` and
`scripts/capture-v2-golden-extra.exs`). Re-capture with:

```sh
# inside the upstream clone, with config/test.exs pointing at a clean database
MIX_ENV=test mix run scripts/../capture_v2_golden.exs
```

## Fixture

The fixture is deterministic and is reproduced exactly by `fixture.json`, which
contains the resulting `users`, `currencies`, `assets` and `discord_users` rows.
It is built with the same operations the Elixir test suite uses:

| Step | Operation |
| --- | --- |
| 1 | create currency `nyan` (guild `900000000000000001`) for discord user `100000000000000001`, creator amount `200000` |
| 2 | create currency `wan` (guild `900000000000000002`) for discord user `100000000000000002`, creator amount `200000` |
| 3 | pay `500` `nyan` from user 1 to user 2 |
| 4 | give `500` `nyan` from the pool to user 2 in guild 1 |
| 5 | ensure a `discord_users` row exists for user 1 |

Balances after setup: user 1 has `199500` `nyan`; user 2 has `1000` `nyan` and
`200000` `wan`. Pool amounts are `500` (`nyan`) and `1000` (`wan`).

## Caveat: the Discord layer is injected

Two `Discord.Api.OAuth2` functions were patched in the throwaway clone, because
neither is injectable (unlike `Discord.Api.Raw`, which the `DiscordApiService`
plug can swap) and both would otherwise perform live Discord requests:

- `get_user_info/1` returns a fixed Discord user payload, including an
  `extra_field_that_must_be_filtered`;
- `refresh_token/1` returns a fixed OAuth2 client whose `access_token` is a JSON
  document, matching the shape the upstream code decodes.

The goldens therefore characterize the controller, the serializer and the refresh
bookkeeping given controlled Discord responses — not Discord itself.

## Contract facts established by these goldens

- `400 {"error":"invalid_request"}` when the `Authorization` header is missing.
- `401 {"error":"invalid_token"}` for a malformed token **and** for a correctly
  signed token whose `jti` row has been deleted.
- `Guardian.verify_claims/2` only checks that a `user_access_tokens` row exists
  for the token's `jti`; it does **not** compare `expires` to now. Expired rows
  are removed by a cron purge instead.
- `plug :accepts, ["json"]` rejects an `Accept` that cannot be satisfied with
  JSON with `406 {"errors":{"detail":"Not Acceptable"}}`. A missing header is
  treated as `*/*`, and `application/json, text/html` is accepted.
- `GET /users/@me` returns `{"id": "<virtualCrypto user id as a string>",
  "discord": {...}}`, where `discord` is `Map.take/2` over the Discord payload
  for these nine keys: `id, username, discriminator, avatar, bot, system,
  mfa_enabled, premium_type, public_flags`. Keys present with a `null` value are
  kept; unknown keys are dropped.
- `GET /users/@me/balances` returns an array of
  `{"amount": "<string>", "currency": {"name", "unit", "guild": "<string>",
  "pool_amount": "<string>"}}`, ordered by **currency id** — not by asset id
  (user 2's assets are ids 2 and 3, yet `nyan` (currency 1) is listed first).
- **Discord token refresh** (`v2_users_me_refresh.json`): when the stored
  authorization is within 15 minutes of its seven-day lifetime (or already
  overdue), `DiscordAuth.refresh_user/1` redeems the refresh token and the
  controller uses the **new** token. The row is updated with
  `Ecto.Repo.update_all/2`, so `token`, `refresh_token` and `expires`
  (`now + expires_in`) change while **`updated_at` is left untouched** — the
  golden records the before/after row under `extra`.

## Intentional deviations from the captured Elixir behaviour

Phoenix-only response headers are **not** replicated. The Rust service does not
reproduce the `; charset=utf-8` parameter, `cache-control: max-age=0, private,
must-revalidate`, or the generated `x-request-id`. None of them is part of the
API contract, and they are all additions a client cannot depend on having. The
one consequence that matters is that auth-error responses, which Elixir wrote
with `Plug.Conn.send_resp` and therefore without any `content-type`, do carry a
JSON `content-type` here.

Comparisons therefore check only status, parsed body, and that the content-type
is JSON. No status code or body differs from Elixir.

## Comparing against these goldens

`assert_matches_golden` in `tests/support/mod.rs` implements the rule above:
status and body must match exactly, and the content-type must be
`application/json` (with or without parameters). The `link` header is not
involved here (no pagination on these endpoints).
