# Deploying this

The contract between the server and whatever runs it: what it needs, what it
serves, and how the schema gets there. Written down because a Dockerfile is only as
correct as this list.

## What the server requires

**The names are this service's own**: the `VCRYPTO_*` secrets `virtualcrypto-prod`
carries, plus `DATABASE_URL` for sqlx. Browser authorization uses temporary request-bound cookies and does not require `SECRET_KEY_BASE`.
The port reads them as they are — there is no second spelling of them to keep in step.
Two of them are older than the port: `VCRYPTO_WEBHOOK_PROXY_CERT` and
`VCRYPTO_WEBHOOK_PROXY_KEY` are the names the Elixir itself read from the environment
(`config/runtime.exs`). The rest were config-file values there, so a name for them in
the environment is one this service introduced.

This is what `require_env` refuses to start without, each named in the error it exits
with.

- `DATABASE_URL`
- `VCRYPTO_API_JWT_SECRET_KEY`
- `VCRYPTO_BOT_TOKEN`
- `VCRYPTO_CLIENT_ID`
- `VCRYPTO_CLIENT_SECRET`
- `VCRYPTO_PUBLIC_KEY`

`VCRYPTO_PUBLIC_KEY` must be 32 hex-encoded bytes; anything else is refused by name.

All seven are already set on the app (`flyctl secrets list -a virtualcrypto-prod`), so
a deployment of this port has nothing to add here.

## What it takes with a default

Already on the app:

- `PORT`
- `VCRYPTO_DISCORD_CALLBACK_URI`
- `VCRYPTO_INVITE_URL`
- `VCRYPTO_SITE_URL`
- `VCRYPTO_SUPPORT_GUILD_INVITE_URL`
- `VCRYPTO_WEBHOOK_PROXY_CERT`
- `VCRYPTO_WEBHOOK_PROXY_KEY`

This port's own — what the Elixir has no equivalent of, and what a deployment of this
port is the first to set:

- `RATE_LIMIT_PER_MINUTE`
- `SECURE_COOKIES`
- `VCRYPTO_REQUEST_SURGE_PER_MIN`
- `VCRYPTO_SECURITY_WEBHOOK_URL`
- `VCRYPTO_SECURITY_WINDOW_SECS`
- `VCRYPTO_SETTLE_INTERVAL_SECS`
- `VCRYPTO_WATCH_LIMIT`
- `VCRYPTO_WATCH_WINDOW_SECS`
- `WEBHOOK_PROXY_URL`
- `WEB_ROOT`
- `VCRYPTO_ENV` (`production` by default; `development` is the only other value)

`WEB_ROOT` is the one that matters to a deployment of the frontend: it is where the
built SPA is unpacked. Unset, `default_web_root()` decides, and `web/dist` is what
`npm run build` writes — so a container has to place one where the other expects it.

`RATE_LIMIT_PER_MINUTE=0` turns the limit off. `WEBHOOK_PROXY_URL` is the one entry
above that a deployment does not have to provide: the emitter is a published endpoint
of this project's, routed by its Worker to `vcrypto-webhook-emitter.sumidora.com` for
every environment (see `wrangler.toml` in
`virtualCrypto-discord/webhook-emitter-cf-workers`), so it defaults like
`VCRYPTO_SITE_URL` does.

The `VCRYPTO_SECURITY_*` and `VCRYPTO_WATCH_*` entries are the behaviour warnings:
this service watches the refusals it already makes (an `invalid_token`, a failed
`same_origin` check, a signature Discord would not have signed, a 429 against the
loose limit) plus its own error rate and request volume, counts them per subject for
`VCRYPTO_SECURITY_WINDOW_SECS` (default 300) and warns once per subject per window.
Warnings always go to the log under the tracing target `vc_security`, so
`RUST_LOG=vc_security=warn` collects exactly these; `VCRYPTO_SECURITY_WEBHOOK_URL`,
when set, is where they are also POSTed — one line for a Discord webhook URL, one
line for a Slack one, structured JSON for anything else. Unset (or blank), it is the
log alone. `VCRYPTO_WATCH_LIMIT` (default 30) over `VCRYPTO_WATCH_WINDOW_SECS`
(default 10) is a second, tighter window held beside the loose limit purely to notice
a burst *before* it becomes a 429; `VCRYPTO_WATCH_LIMIT=0` disables it.
`VCRYPTO_REQUEST_SURGE_PER_MIN` (default 3000) is what total request volume has to
exceed before that is warned about; zero disables it. None of this refuses a
request — it only tells the operator.

Webhook reports are committed to `security_webhook_queue` before enqueueing
returns (migration `0022`). A worker resumes pending rows on startup and sends
them in order, with a database lock preventing concurrent delivery by multiple
machines. The worker also runs when `VCRYPTO_SETTLE_INTERVAL_SECS=0`.
Network failures and HTTP 408, 429, and 5xx remain queued, with exponential retry
delays from one second up to five minutes. A longer `Retry-After` header or
Discord JSON `retry_after` takes precedence; later reports wait behind it.
Successful rows are deleted immediately. Other HTTP errors are retained as
permanent failures with `failed_at` and `last_status`; the normal purge removes
them after seven days and never removes pending retries.

The queue stores the report, attempt count, and next attempt time, never the
webhook URL. Pending reports use the currently configured URL, including after
credential rotation. Delivery is at least once: a crash after the webhook accepts
a report but before the database commits can produce a duplicate notification.
Fixing a permanent failure's cause and clearing its `failed_at`, with
`next_attempt_at = now()`, makes that row eligible for delivery again.

`VCRYPTO_WEBHOOK_PROXY_CERT` and `VCRYPTO_WEBHOOK_PROXY_KEY` are the mTLS client the
webhook handshake goes through — the Cloudflare Worker in front of an application's
webhook requires a client certificate, which is why the pair is required together
rather than separately. `#` in the PEM values is read as a newline. In production,
both are required: a missing or invalid pair stops startup before any jobs or
HTTP listener start. Neither registration handshakes nor notifications can fall
back to direct requests in production.

Only `VCRYPTO_ENV=development` permits direct requests to an application's
`webhook_url` when both proxy credentials are absent. A partial or invalid pair
is still an error. `scripts/discord-env.sh` selects development by default for
local runs; `fly.toml` explicitly selects production. Keep development mode local,
since its direct transport can reach local and private network addresses.

`VCRYPTO_SITE_URL` must match the public browser origin. OAuth consent submissions
use Fetch Metadata and Origin/Referer headers for CSRF protection, comparing to
this configured origin rather than the request's Host or forwarding headers.

## The schema

```sh
sqlx migrate run --source crates/vc-core/migrations
```

The baseline is a single migration and `just migrate` runs exactly this, so a
deployment has one command to run and one directory to ship.

Migration `0021_discord_interactions.sql` adds durable receipts for Discord
commands, components and modal submissions. The first request claims its
interaction ID before dispatch; duplicates replay the recorded HTTP response.
An in-progress or interrupted request returns 409 on duplicate delivery.

The existing purge job deletes receipts and response bodies after 24 hours
(normally on its next one-minute tick). Requests with a signature timestamp older
than five minutes or more than 30 seconds in the future are refused, so replaying
an old signed request remains ineffective after its receipt is deleted. Keep the
server clock synchronized, and keep the scheduler enabled for automatic cleanup.

Do not delete an unfinished receipt to retry a command: its payment may already
have committed before the process stopped. Check the payment history before
deciding whether another payment is needed. The receipt temporarily stores the
response (including ephemeral response content), but not the incoming
interaction token or request body.

`/pay` first sends a private "処理中…" message (`type: 4`, `EPHEMERAL`) through
Discord's callback endpoint. Only after Discord accepts it does the payment
start; a failed or timed-out acknowledgement does not transfer funds. Success is
sent as a public follow-up, then the private processing message is deleted.
Errors replace the private message. If publishing or deleting fails after a
successful payment, the private message is updated with the known success.
Notification failures do not retry the payment.

`/issue` also acknowledges with a private "処理中…" message before starting the
issuance. Both success and error replace that private message. A rejected or
timed-out acknowledgement never issues currency, and a notification failure
never retries issuance.

`/claim make`, `/claim approve`, `/claim deny`, and `/claim cancel` use the same
private processing message and replace it with the result. Claim decision buttons
and contract approval, refusal, and withdrawal buttons first acknowledge with
`type: 6` (deferred message update), then perform the operation and edit their
existing private screen. Claim list decisions also send the outcome as a private
follow-up. If that follow-up fails, the outcome replaces the private screen; a
failed list redraw does not discard the operation's result. A single-claim refusal
is sent privately without changing its screen, with the same edit fallback.
All these mutations start only after a successful acknowledgement, and result
delivery failures never repeat them.

The same ordering applies to `/create`, `/pat create|revoke`, `/mute` and
its removal buttons, `/application register`, and currency deletion submissions.
They acknowledge privately and replace the message with the result. Grant
approval/revocation, mute removal buttons, and application setting/bot selectors
acknowledge with `type: 6` before updating their private screen. A separate private
refusal remains a follow-up, with an edit fallback if delivery fails. Every
application settings form submission uses the existing private `type: 5` path,
including edits with no webhook handshake. Buttons opening modals and read-only
navigation keep their inline responses.

These paths return an empty HTTP 202 after acknowledgement. Its receipt records
acceptance, so replaying that 202 never restarts the operation, including while it
is still running. Long database waits happen after the initial Discord response.

## Runtime timeouts

The runtime database pool sets `statement_timeout` to 30 seconds on every new
connection, including replacement connections. This bounds queries and row-lock
waits; schema migrations still run through the separate migration command.
The v2 API also applies a 30-second request timeout before authentication and body
parsing. It returns `504 {"error":"request_timeout"}` on timeout; a committed
write is not undone, so clients should retain their idempotency keys on retries.

Webhook deliveries have a five-second timeout and a 64 KiB response-body limit,
for both proxy and direct transports. The limit also applies while streaming
responses without Content-Length. Oversized bodies are discarded and cannot
satisfy a successful PING check; refusal status codes still retain their meaning.

## The frontend

`web/dist` is not committed; CI builds it (`web` job) and a deployment packages the
result. The API serves it as a **fallback** rather than a mount, so `/api`, `/health`
and the OAuth2 routes keep their own paths, and the two cache headers that come with
that are tested in `crates/vc-api/tests/web.rs`.

## Still missing

No `Dockerfile` and no target-specific configuration. What is above is the input to
writing one, not the thing itself.
