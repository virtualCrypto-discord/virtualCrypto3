# Known gaps

Behaviour that the Elixir service has and this rewrite does not implement yet,
listed here so it cannot be forgotten. Everything here is deliberately deferred;
nothing is dropped by accident.

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

- The pagination `link` header on `GET /api/v2/users/@me/claims` is implemented
  from the controller source (`build_url_from_options/2`): scheme from
  `x-forwarded-proto`, host from the `Host` header with any port removed, and the
  query rebuilt from the options. The tests assert only that a full page carries
  the header and a partial page does not — the exact URL has not been captured
  from Elixir because its test suite does not exercise pagination.
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
