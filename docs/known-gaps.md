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
only `user` and `app`, so a `guild` token is rejected today.

**No longer true, and kept here because the shape is deliberate.** The rewrite
implements the `guild` kind the specification asks for, but the claim is not what
carries it: the token a guild issues for is the `access_tokens` row the code flow
has always handed out, and its guild-ness is what that row resolves to
(`vc_core::grant::resolve_token`). The kind check `verify_claims/2` would grow is
instead answered by the grant — see `docs/issue.md` for the endpoint, the scope,
and the ask a guild answers. If `guild` tokens are ever issued *as* JWTs,
that check still has to grow with them.

Behaviour that the Elixir service has and this rewrite does not implement yet,
listed here so it cannot be forgotten. Everything here is deliberately deferred;
nothing is dropped by accident.

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

The v2 REST API takes the same extractor an endpoint does when it needs it, in
`routes/limited.rs`: `Limited(AuthUser)` counts per account `v2:{subject}`, and
the guild endpoints count per application `guild:{application_id}` in the guild
token's own extractor. The coverage is still uneven — an endpoint is limited when
its extractor is one of those, and the limiter has to be asked for where the
token's identity is — but the per-account shape is where it lands.

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

## Deliberate differences

### The currency command is `/issue` here, where the Elixir's was `/give`

Renamed, not reimplemented. The Elixir registered `give` in
`priv/register-commands.exs` and dispatched it in `Command.handle/4`, but the call
behind it was `Query.Issue.issue/3` all along — so this service registers and
dispatches `issue`, and the domain function is `vc_core::issue::issue`. The
options, the administrator bit, the guild requirement and the answers are
unchanged; `tests/interactions_issue.rs` is the port, and `docs/test-port.md`
records that Elixir had no test for it either way.

The rename stops at the command. The table the operation writes keeps the Elixir's
name, `currency_given_histories`, because the baseline is the v2 Ecto schema and
both services share it.

Registering the new name is what removes the old one — Discord's `PUT` to
`/applications/{client_id}/commands` is a bulk overwrite, so `just register-commands`
puts `/issue` up and takes `/give` down in one call. Until it is run Discord still
offers `/give`, and an interaction for it is answered `400 Type Not Found`, since no
`Command.handle/4` clause matches that name any more.

### The login redirect is 303 where Phoenix sent 302

`Redirect::to` in axum is `303 See Other`; `Phoenix.Controller.redirect/2` sends
`302 Found`. For a browser navigating a `GET` the two behave identically, which
is why this was accepted rather than hand-building a 302 — and for the consent
screen's `POST`, which is the case that follows, 303 is the correct answer rather
than merely an acceptable one.

### `POST /token` without a session answers 401

The Elixir controller's `token/2` returned `nil` when the session had no user,
which in Phoenix is not a response at all — the request fails rather than being
answered. Nothing depended on that, and a status is something a caller can act
on, so this answers `401` with the same `invalid_token` body the rest of the API
uses.

### A member's unknown role id is skipped, where the Elixir crashed

`validate_executor` maps the member's role ids through the guild's roles and calls
`String.to_integer/1` on each result, so a role id the guild does not list raises.
That is reachable, not theoretical: the member and the guild's roles come from two
separate Discord calls, and a role deleted between them lands exactly here — which
makes it a bug rather than a branch, and one that a retry would not fix.

The Rust side skips the id, so the same race costs a permission check that may be
a moment stale instead of the request.

### The consent screen's two repairs

`validate_executor` asked Discord about the session's user id, which is a
VirtualCrypto id and not a Discord one, so the lookup could not succeed and the
Approve button was unreachable. It now reads the account's `discord_id` and asks
with that.

A missing session raised, which is a 500 where the answer is a trip to
`/login?continue=…`. That repair needed the login page to exist, which is why it
waited until it did.

### `POST /oauth2/token` answers 400 where the Elixir answered 200

`TokenController` renders the code exchange's error bodies without setting a
status, so they arrive as `200` with an error in the body. RFC 6749 has
`invalid_grant` at `400`, and a client library that checks the status before the
body — which is most of them, and which is what this endpoint exists for — reads
the Elixir's answer as a success. The refresh and `client_credentials` paths in
the same controller do set `400`, so the code exchange was the odd one out.

## The notification transport, as far as it is specified

`docs/api/Webhook.md` and `notification/webhook/cloudflare-workers.ex` together
say what the delivery is, and it is smaller than the earlier note in this file
assumed.

**The protocol** is Discord's, deliberately. Events are a claim being approved or
denied; a body is `{ "type": 1 | 2, "data": ... }` with 1 as PING and 2 as a
claim update, and **unknown types must be ignored**. The signature is ed25519 over
the timestamp and the body concatenated as bytes, in `X-Signature-Ed25519` as hex
and `X-Signature-Timestamp` as digits — the same thing this service already
verifies on its own interaction endpoint, which is why `ed25519_dalek` is already
in the tree. An application that cannot verify must answer `401`, and
**VirtualCrypto checks that it can at registration and periodically afterwards**.

**The delivery goes through a Cloudflare Worker, and that worker is protected by
mTLS** — so a client certificate belongs on this side, and an earlier version of
this paragraph was wrong to say otherwise. The Elixir reads a `webhook_proxy` from
configuration and posts there; the worker presents a certificate of its own and
requires one from this side. That is the whole of the protection, so a client
without the certificate is not a client the worker will talk to.

`reqwest` takes one as an identity — `Identity::from_pem` over the certificate and
its key together — and that needs the TLS feature to be chosen deliberately rather
than left at the default.

The next thing to read is that worker's own request shape; its source is
`webhook-emitter-cf-workers` in the same organisation.

**The keys are per application**, and are the reason `applications` has both
`public_key` and `private_key` as `NOT NULL`: the service signs what it sends with
the private half and the application verifies with the public one. That is also
why the fixture in `tests/oauth2_preauthorize.rs` has to write both, which the
runtime taught us rather than the schema.

The delivery is implemented: `WebhookNotifier` signs with the application's
private key and posts through the worker where one is configured, and straight at
the application's own `webhook_url` where there is none — the same choice the
handshake makes. What is missing here is the re-verification: `verify` runs when
a webhook is registered or edited, and nothing runs it afterwards, because there
is no scheduler in this service at all.

### What a delivery is, and what the handshake checks

Read out of `CloudflareWorkers`. The proxy is generic: **the target is in the
request**, and the proxy answers with what the target said.

A delivery is a `POST` to the configured proxy whose body is the event's JSON and
whose headers are:

| Header | |
| --- | --- |
| `X-Signature-Ed25519` | ed25519 over `timestamp <> body`, **hex and lowercase** |
| `X-Signature-Timestamp` | the time in whole seconds |
| `X-Forward` | **the application's `webhook_url`** — where the proxy should send it |

and the signature is made with the **application's** `private_key`, not with
anything of this service's. The response carries `X-Status`, the status the
target answered with, plus the target's body — so the caller learns what the
application said without the proxy having to paraphrase it.

The handshake is **two requests, not one**, and that is the part worth not
simplifying:

1. a PING — `{"type": 1}` — signed with the application's real key must come back
   `200` and a body of `{"type": 1}`, which says the application answers webhooks
   at all;
2. a PING signed with a **keypair generated for the occasion** must come back
   `401`, which says the application actually verifies what it is sent.

An application that cannot tell a real signature from rubbish fails the second and
is refused. Two results must both be `:ok`; a wrong answer is
`verification_failed`, a proxy that could not be reached is
`internal_server_error`. This service draws the same line in its own words —
`webhook_verification_failed` for the application's failure, `server_error` for its
own — and a silence with no proxy in the way is the application's, because there is
then nothing between this service and the webhook for it to belong to.

The two requests are **shuffled** so that the order is not something to guess at,
and `verify` draws which PING goes first for that reason: an application that
answers `200` to the first request and `401` to the second, checking nothing,
passes only the handshakes whose real PING happened to be the one that went first.

The handshake is also rate-limited **per requester** — one per three seconds,
twenty per hour, fifty per day — answering `retry_after_3_seconds`,
`retry_after_1_hour` or `retry_after_1_day`. `rate_limit::VerificationLimiter` is
that limiter, charged to the account that would be asking twice, by registration
and by the edit.

The client certificate lives in this module's own configuration under `:ssl`,
which is the mTLS the worker demands. And a notification is **fire-and-forget**:
`Task.start`, so a slow application does not slow the claim that caused it, with
an account that has no application being a silent `:nop`.

## The v2 rate limit, and why it is an extractor

The limiter already exists and the interaction endpoint already uses it:
`RateLimiter::allow(&self, key: &str) -> bool`, keyed per caller — the interaction
side asks for `format!("discord:{user}")`. What v2 has not got is the calling of
it, and where to call it decides how much code that is.

A layer on the v2 router is the wrong place. The limit is per **account**, and an
account is only known once the token has been verified — which happens inside the
handlers, through the `AuthUser` extractor. A layer would therefore have to verify
tokens a second time, or limit on the address, and the Elixir counts the caller.

The right place is an extractor of the same kind: one that verifies the token and
then asks the limiter, so a handler takes `Limited` instead of `AuthUser` and
cannot forget to be limited. It has to be `FromRequestParts<AppState>` rather than
generic over `AuthState`, because the limiter is the state's and `AuthState` — the
pool and the signing secret — does not carry it.

That is a parameter change in every v2 handler: mechanical, about twenty of them,
and the compiler finds them all. The refusal is the one the interaction endpoint
already answers with, and `RATE_LIMIT_PER_MINUTE=0` still turns it off.

