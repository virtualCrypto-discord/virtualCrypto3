# Deploying this

The contract between the server and whatever runs it: what it needs, what it
serves, and how the schema gets there. Written down because a Dockerfile is only as
correct as this list.

## What the server requires

**The names are the deployment's, not this code's.** They were read from
`flyctl secrets list -a virtualcrypto-prod`, because that is what the running service
actually has: the values are already there, so a port that invented its own names
would exit at startup naming variables nobody has. `VCRYPTO_*` is the older
service's spelling and it is the one to keep.

The first list is what `require_env` refuses to start without, each named in the
error it exits with; the second is everything else that is read, `WEB_ROOT` from
`vc-api` rather than from `main.rs`.

- `DATABASE_URL`
- `VCRYPTO_BOT_TOKEN`
- `VCRYPTO_CLIENT_ID`
- `VCRYPTO_CLIENT_SECRET`
- `VCRYPTO_PUBLIC_KEY`
- `VCRYPTO_API_JWT_SECRET_KEY`
- `SECRET_KEY_BASE`

`VCRYPTO_PUBLIC_KEY` must be 32 hex-encoded bytes; anything else is refused by name.

## What it takes with a default

- `VCRYPTO_INVITE_URL`
- `PORT`
- `RATE_LIMIT_PER_MINUTE`
- `SECURE_COOKIES`
- `VCRYPTO_SITE_URL`
- `VCRYPTO_SUPPORT_GUILD_INVITE_URL`
- `VCRYPTO_WEBHOOK_PROXY_CERT`
- `VCRYPTO_WEBHOOK_PROXY_KEY`
- `WEBHOOK_PROXY_URL`
- `WEB_ROOT`

`WEB_ROOT` is the one that matters to a deployment of the frontend: it is where the
built SPA is unpacked. Unset, `default_web_root()` decides, and `web/dist` is what
`npm run build` writes — so a container has to place one where the other expects it.

`RATE_LIMIT_PER_MINUTE=0` turns the v2 limit off. `WEBHOOK_PROXY_URL` is the one entry here that a deployment does not have to
provide: the emitter is a published endpoint of this project's, routed by its
Worker to `vcrypto-webhook-emitter.sumidora.com` for every environment (see
`wrangler.toml` in `virtualCrypto-discord/webhook-emitter-cf-workers`), so it
defaults like `SITE_URL` does. What decides whether there is a proxy at all is the
certificate below.

It goes together with
`VCRYPTO_WEBHOOK_PROXY_CERT` and `VCRYPTO_WEBHOOK_PROXY_KEY` is the mTLS client the
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

