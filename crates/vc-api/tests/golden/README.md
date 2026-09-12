# v2 golden responses (captured from the Elixir implementation)

These files pin the behaviour of the two `/api/v2` endpoints that have **no test
coverage** in the Elixir suite, so the Rust rewrite has an executable contract:

- `GET /api/v2/users/@me`
- `GET /api/v2/users/@me/balances`

Each file records the status, the response headers and the parsed JSON body for a
fixed fixture. They were produced by running the Elixir application in `test`
mode against the Ecto-migrated database and dispatching requests through
`Phoenix.ConnTest` (see `scripts/capture-v2-golden.exs`). Re-capture with:

```sh
# inside the upstream clone, with config/test.exs pointing at a clean database
MIX_ENV=test mix run scripts/capture-v2-golden.exs
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

## Caveat: the Discord payload is injected

`GET /api/v2/users/@me` calls `Discord.Api.OAuth2.get_user_info/1`, which performs
a live Discord API request and is **not** injectable (unlike `Discord.Api.Raw`,
which the `DiscordApiService` plug can swap). For the capture, that one function
was patched in the throwaway clone to return a fixed Discord user payload
including an `extra_field_that_must_be_filtered`. The goldens therefore
characterize the controller and serializer given a controlled Discord response,
not Discord itself.

## Contract facts established by these goldens

- **Auth failures carry no `content-type` header in the captured Elixir
  responses.** Both the `400` and the `401` are written with
  `Plug.Conn.send_resp`, which sets neither content-type nor a JSON header; only
  `cache-control` and `x-request-id` are present. The Rust implementation
  deliberately deviates here — see below.
- `400 {"error":"invalid_request"}` when the `Authorization` header is missing.
- `401 {"error":"invalid_token"}` for a malformed token **and** for a correctly
  signed token whose `jti` row has been deleted.
- `Guardian.verify_claims/2` only checks that a `user_access_tokens` row exists
  for the token's `jti`; it does **not** compare `expires` to now. Expired rows
  are removed by a cron purge instead.
- Successful responses use `content-type: application/json; charset=utf-8` and
  `cache-control: max-age=0, private, must-revalidate`.
- `GET /users/@me` returns `{"id": "<virtualCrypto user id as a string>",
  "discord": {...}}`, where `discord` is `Map.take/2` over the Discord payload
  for these nine keys: `id, username, discriminator, avatar, bot, system,
  mfa_enabled, premium_type, public_flags`. Keys present with a `null` value are
  kept; unknown keys are dropped.
- `GET /users/@me/balances` returns an array of
  `{"amount": "<string>", "currency": {"name", "unit", "guild": "<string>",
  "pool_amount": "<string>"}}`, ordered by **currency id** — not by asset id
  (user 2's assets are ids 2 and 3, yet `nyan` (currency 1) is listed first).

## Intentional deviations from the captured Elixir behaviour

These are the only places the Rust implementation is allowed to differ from the
goldens. Both are non-breaking: no status code, body or existing header changes.

| Deviation | Reason |
| --- | --- |
| Auth-error responses (`400`/`401`) gain `content-type: application/json` | The Elixir responses omit it only as an artefact of writing the body with `Plug.Conn.send_resp`; adding the header cannot break a client that already parsed the body. |
| `x-request-id` is generated per request and ignored when comparing | Not part of the contract. |

The Rust contract tests therefore expect `content-type: application/json` on
auth errors, while the differential harness (see the plan) must allow-list this
one header difference against the Elixir service. Optionally the Elixir
implementation could be patched to send the same header before cut-over, which
would make both sides identical.

## Comparing against these goldens

`x-request-id` is generated per request and must be ignored when diffing. The
`link` header is not involved here (no pagination on these endpoints).
