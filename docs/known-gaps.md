# Known gaps

## Where the contract comes from

`https://github.com/virtualCrypto-discord/virtualcrypto-docs` is the official
specification (`docs/api/Rest.md`, `Authz.md`, `Webhook.md`). Two clauses there
shape how everything is verified:

- *"未知のフィールドは無視しなければなりません(レスポンスへのフィールドの追加や、
  リクエストへの Optional なフィールドの追加は破壊的な変更とみなされません)"* —
  unknown fields are ignored, and adding response fields or optional request
  fields is not a breaking change.
- *"`error_description` フィールドの内容、およびその存在の有無は仕様の範囲外です"* —
  the **content and even the presence** of `error_description` is explicitly out
  of scope. Only `error` / `error_info` and the status code are contractual, so
  the exact wording and ordering of messages such as the metadata validation
  details are not chased. (This is why `Rest.md` can document
  `forbidden`/`invalid_operator` for `GET /claims/:id` while the implementation
  returns `not_related_user`; both are conformant.)

`Rest.md` does not document `GET /api/v2/users/@me` or
`GET /api/v2/users/@me/balances` at all, and the reason is that they are consumed
only by the web frontend. Their JSON is therefore an *internal* shape rather than
a published contract: the goldens are a convenience, not a promise, and both
endpoints may change or disappear alongside the frontend rewrite. Do not spend
fidelity effort on them the way the documented endpoints deserve.

`Authz.md` documents three token kinds — `user`, `app` and `guild` — and says a
`guild` token is what may `give`. The implementation's `verify_claims/2` accepts
only `user` and `app`, so a `guild` token is rejected today; the rewrite mirrors
the implementation. If `guild` tokens are ever issued, the kind check has to
grow with them.

Behaviour that the Elixir service has and this rewrite does not implement yet,
listed here so it cannot be forgotten. Everything here is deliberately deferred;
nothing is dropped by accident.

## Notifications for claim status changes

When a claim is approved or denied, Elixir calls
`VirtualCrypto.Notification.Dispatcher.notify_claim_update/2`, which resolves the
claimant to their application and POSTs an Ed25519-signed event to the
application's `webhook_url` through the Cloudflare Workers proxy (mutual TLS,
`X-Signature-Ed25519` / `X-Signature-Timestamp` headers, `X-Forward` target).

The Rust service builds and dispatches that event — `vc_core::notification::Notifier`
is the seam, the payload matches `format_claim_for_notification/1`, and
`tests/notification.rs` pins it against the same assertions the Elixir suite
makes. **What is still missing is transport**: the server runs with
`NoopNotifier`, so nothing actually reaches an application. That half needs the
OAuth2/application side of the domain, which does not exist here yet:

- the `applications` row's `webhook_url`, `public_key` and `private_key`;
- `VirtualCrypto.Exterior.User.Resolver` to turn the claimant into an application;
- an HTTP client able to present the webhook proxy's client certificate
  (`VCRYPTO_WEBHOOK_PROXY_CERT` / `VCRYPTO_WEBHOOK_PROXY_KEY`);
- the `Hammer` rate limits on webhook verification (1 per 3s, 20 per hour,
  50 per day, per requester);
- `verify/4`, the webhook-URL verification handshake used when an application is
  registered or patched.

Because it is entangled with the OAuth2 provider milestone, it is scheduled
there rather than in the claim endpoints.

## Discord lookups are cached in the process

`Discord.Api.Cached` wraps the raw API and remembers `get_user` and `get_guild`
for fifteen minutes, a 404 included — the same shape as the Elixir `Cachex`
tables, and what keeps a page of claims from asking about the same missing user
once per row.

Like Elixir's, a miss is fetched once: callers that want the same id while it is
being looked up wait on that id's lock and then read what the first one stored,
rather than each making their own call.

Two differences from Elixir's, both about the bound rather than the behaviour:

- The table is bounded (ten thousand entries) and gives up its oldest tenth
  rather than clearing, so a long-lived server cannot grow without limit and a
  full table does not send every caller back to Discord at once.
- It lives in this process, so every Fly machine keeps its own. That changes
  nothing a caller can see.

## Rate limiting is loose, and only on the interactions endpoint

Requests are counted per identity rather than per address, because an address
says nothing once authentication has decided who is calling: `RATE_LIMIT_PER_MINUTE`
(default 120, zero disables it) applies to the Discord user behind an
interaction, established by the signature check that runs before it.

The v2 REST API is not covered yet. Its identity is the token's subject, which
the `AuthUser` extractor already resolves, so the limiter needs to be held there
the same way.

## Not yet verified against captures

- The pagination `link` header on `GET /api/v2/users/@me/claims` follows the
  example in `Rest.md`: `<scheme>://<Host verbatim, port included><path>?…`, with
  the query rebuilt as `type`, `order`, `next`, `limit`, `related_*`, then
  `statuses[]`. The tests assert that a full page carries the header and a partial
  page does not; asserting the exact URL is still to do.
- `related_discord_user_id` resolves an existing user, while Elixir's resolver
  may create one. A freshly created user has no claims, so the filter result is
  the same either way, but the side effect differs.
- An unparsable cursor value or an unknown `order` answers 500, matching the
  crashes Elixir hits in `parse_order/1` and in Ecto's `bigint` cast. These are
  client errors that the Elixir service mishandles; they are reproduced rather
  than invented differently.

## Documented deviations from captured behaviour

These are intentional and non-breaking; each is also recorded next to the
relevant test.

- Response headers that are incidental to Phoenix (the `charset` parameter,
  `cache-control`, `x-request-id`) are not replicated.
- Auth-error responses gain a JSON `content-type`, which Elixir omitted only
  because it wrote them with `Plug.Conn.send_resp`.
- `GET /api/v2/users/@me/claims/:id` answers `404` for a non-numeric id, where
  Ecto's `bigint` cast would raise a 500. `PATCH` already answers an unparsable
  id with 404, so the two endpoints stay consistent.
- The `APPLICATION_COMMAND_PERMISSIONS_V2` branch is gone. Elixir required the
  administrator bit only from guilds carrying that feature, and let every other
  guild run `create` and `give`; Discord finished that migration, so no such
  guild is left and the branch could no longer be false. Removing it also
  removes the guild lookup those two commands made solely to read the feature
  list. `create_test.exs`'s "not admin without v2 flag" case passes the default
  permissions — every bit set — so it still passes; its name is the only thing
  that referred to the flag.
- The claim list's `:last` page is resolved by counting the matching claims, so
  it really is the last page. Elixir's `Raw.Get` clause for `%{page: :last}`
  fetches one page's worth of rows in ascending order and reverses them, which
  only lands on the last page when there is a single one — with more, it
  re-renders the first. No Elixir test covers it, and the button that asks for
  it is disabled while there is one page, so this diverges from a path the suite
  does not pin.

## Deliberately out of scope

Two items in the original plan were dropped by decision, not forgotten:

- `GET /api/v2/users/@me/balances` is not implemented. It and `/users/@me` are
  consumed only by the web frontend, which is being rebuilt separately, so the
  endpoint waits for that. (`/users/@me` is implemented because it already
  existed.)
- The differential harness — replaying a recorded request corpus against both the
  Elixir and the Rust service — is not built. The ported contract tests and the
  captured goldens are the evidence of compatibility instead.

CI is no longer one of these. `.github/workflows/ci.yml` brings up a Postgres 17
— the major version the baseline was generated from — and runs `just check`,
which is the same four gates a laptop runs. The database is what the `sqlx`
macros compile against, so no `.sqlx` offline cache is committed and none can go
stale.

## OAuth2 and applications

Not built at all: no `/oauth2/*` route exists, so no third-party application can
register or obtain a token. This is the largest remaining milestone, and unlike
everything else in this migration Elixir has **no tests for it**, so there is no
ported spec — `docs/oauth2.md` records the contract read out of the controllers,
which is where a port would have to start.

It is also what the claim notifications are waiting on: the webhook handshake
that registering an application performs is the same mutual-TLS path the
notifications are sent over, so the transport arrives with this.

## Discord interactions

`POST /api/integrations/discord/interactions` verifies the Ed25519 signature over
`timestamp <> body` and answers PING (type 1) with a PONG. Types 2 (application
commands), 3 (message components) and 5 (modals) are dispatched by name or by
`component_type`, with the state each one acts on packed into its `custom_id`.
Type 4 (autocomplete) is dispatched on the focused option's name and, for a
claim id, the command path — the same `id` offers received-and-pending under
`approve` and `deny`, claimed-and-pending under `cancel`, and every status under
`show`. Elixir has no test for any of it, so `tests/interactions_autocomplete.rs`
holds additions rather than ports.

One thing it cannot name yet: a claim whose party is an application rather than a
discord user reads as `deleted`, where Elixir shows that application's client
name. That needs the OAuth2/application side.

One deviation: a command name that no `Command.handle/4` clause matches answers
400 `Type Not Found`, where Elixir has no clause either and so raises, producing
a 500. Discord only sends registered command names, so this is unreachable in
practice; it is recorded because it is not a faithful reproduction.
