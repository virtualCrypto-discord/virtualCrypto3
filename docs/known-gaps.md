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

## Bulk payments

`POST /api/v2/users/@me/transactions` also accepts an array body in the Elixir
service, paying every entry inside one transaction and reporting a bad element as
`invalid_amount_at_<index>`, `invalid_unit_at_<index>` or
`invalid_receiver_discord_id_at_<index>`.

**The Rust service implements only the single-object form.** An array body
currently answers 500 rather than half-working, so the twelve cases in
`v2/user_transactions/pay/bulk/bulk_user_transacion_controller_test.exs` and the
one in its idempotency sibling are still to port.

## Notifications for claim status changes

When a claim is approved or denied, Elixir calls
`VirtualCrypto.Notification.Dispatcher.notify_claim_update/2`, which resolves the
claimant to their application and POSTs an Ed25519-signed event to the
application's `webhook_url` through the Cloudflare Workers proxy (mutual TLS,
`X-Signature-Ed25519` / `X-Signature-Timestamp` headers, `X-Forward` target).

**The Rust service does not send these notifications.** Claim status transitions
therefore complete without notifying anyone. This is the most significant
functional gap in Milestone 1.

Bringing it in requires, from the OAuth2/application side of the domain:

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

## Discord lookups are not cached

Elixir wraps `Discord.Api.Raw` in `Discord.Api.Cached`, backed by `Cachex` with a
15 minute TTL, so repeated claim serialization does not re-query Discord. The
Rust `DiscordApi` calls Discord on every lookup. Responses are identical; only
Discord API traffic and latency differ.

## No rate limiting

`Hammer` is only used for webhook verification in Elixir, so nothing is missing
yet. It becomes relevant together with notifications.

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
