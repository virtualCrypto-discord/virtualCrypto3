# Deploying this

The contract between the server and whatever runs it: what it needs, what it
serves, and how the schema gets there. Written down because a Dockerfile is only as
correct as this list.

## What the server requires

**The names are this service's own**: the `VCRYPTO_*` secrets `virtualcrypto-prod`
carries, plus `DATABASE_URL` and `SECRET_KEY_BASE`, which are Phoenix's and sqlx's.
The port reads them as they are — there is no second spelling of them to keep in step.
Two of them are older than the port: `VCRYPTO_WEBHOOK_PROXY_CERT` and
`VCRYPTO_WEBHOOK_PROXY_KEY` are the names the Elixir itself read from the environment
(`config/runtime.exs`). The rest were config-file values there, so a name for them in
the environment is one this service introduced.

This is what `require_env` refuses to start without, each named in the error it exits
with.

- `DATABASE_URL`
- `SECRET_KEY_BASE`
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
- `VCRYPTO_SETTLE_INTERVAL_SECS`
- `WEBHOOK_PROXY_URL`
- `WEB_ROOT`

`WEB_ROOT` is the one that matters to a deployment of the frontend: it is where the
built SPA is unpacked. Unset, `default_web_root()` decides, and `web/dist` is what
`npm run build` writes — so a container has to place one where the other expects it.

`RATE_LIMIT_PER_MINUTE=0` turns the limit off. `WEBHOOK_PROXY_URL` is the one entry
above that a deployment does not have to provide: the emitter is a published endpoint
of this project's, routed by its Worker to `vcrypto-webhook-emitter.sumidora.com` for
every environment (see `wrangler.toml` in
`virtualCrypto-discord/webhook-emitter-cf-workers`), so it defaults like
`VCRYPTO_SITE_URL` does. What decides whether there is a proxy at all is the
certificate below.

`VCRYPTO_WEBHOOK_PROXY_CERT` and `VCRYPTO_WEBHOOK_PROXY_KEY` are the mTLS client the
webhook handshake goes through — the Cloudflare Worker in front of an application's
webhook requires a client certificate, which is why the pair is required together
rather than separately. `#` in the PEM values is read as a newline. Without the
pair there is no proxy at all, and both the webhook handshake and the deliveries
go straight at each application's own `webhook_url` — the development path,
where a machine has no worker to reach applications through.

## The schema

```sh
sqlx migrate run --source crates/vc-core/migrations
```

The baseline is a single migration and `just migrate` runs exactly this, so a
deployment has one command to run and one directory to ship.

## The frontend

`web/dist` is not committed; CI builds it (`web` job) and a deployment packages the
result. The API serves it as a **fallback** rather than a mount, so `/api`, `/health`
and the OAuth2 routes keep their own paths, and the two cache headers that come with
that are tested in `crates/vc-api/tests/web.rs`.

## Still missing

No `Dockerfile` and no target-specific configuration. What is above is the input to
writing one, not the thing itself.

