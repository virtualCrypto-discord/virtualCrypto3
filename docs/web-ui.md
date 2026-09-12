# The web UI, derived from the site it replaces

The requirements below come from the Elixir app's own pages rather than from
imagination: the router's browser scope, its three LiveViews, and the controllers
behind them. Where something was not read, it says so.

## What the old site is

| Path | What it is | Notes |
| --- | --- | --- |
| `GET /` | landing page | |
| `GET /logout` | ends the session | |
| `GET /invite`, `/support` | redirects | to the bot and the support guild |
| `GET /callback/discord` | the Discord OAuth2 callback | see the session model below |
| `POST /token` | the session's VC API token | re-issues on demand |
| `live /app` | **a stub** | its whole body is `<h1>Content</h1>` |
| `live /me` | the dashboard | the router points at `DashboardApplication` |
| `GET /applications/:id` | one application | renders the JSON *into* an HTML page |
| `live /applications/:id/connect` | the connect flow | two events: `verify`, `change` |
| `live /contract/:id` | approving a contract | |
| `GET /applications/verification` | a readme | |
| `GET /document/{,,about,commands,api}` | four documents | static prose |
| `GET/POST /oauth2/authorize` | the consent screen | needs a session |

Two things worth saying plainly: **`/app` is a stub** — whatever the dashboard is
meant to be, it is not in the old site either; and the documents under
`/document` are prose, so they belong to the docs site rather than to a rebuild.

## The session, read out of the old app

The cookie is **signed, not encrypted** — `Plug.Session` is configured with
`store: :cookie`, `key: "_virtualCrypto_key"`, `signing_salt` and
`same_site: "Lax"`, `secure` in production only — so its contents are readable by
the browser and only tamper-proof. A Rust session should be the same kind of
cookie rather than something more elaborate.

`browser_auth` is an interceptor rather than a guard: a request with no session
is **redirected** to Discord, and so is one whose stored Discord authorization
cannot be refreshed.

The login is three steps:

1. generate a `state` of 32 random bytes, base64-url without padding, store
   `discord_oauth2: {state, continue: <the url that was asked for>}` in the
   session, and redirect to
   `https://discord.com/api/oauth2/authorize` with `client_id`, `redirect_uri`,
   that `state`, `scope=identify` and **`prompt=none`** — Discord shows no
   consent screen to someone who has already agreed;
2. `/callback/discord` compares the returned `state` with the session's, then
   exchanges the code at `https://discord.com/api/oauth2/token`
   (`grant_type=authorization_code`, `client_id`, `client_secret`,
   `redirect_uri`, and `Accept: application/json`);
3. the token is used to read the Discord user, the authorization is stored, and
   a VC API token is issued — see the next section for what happens to it.

An SPA changes one thing and one thing only: step 3's answer. Because
`browser_auth` redirects, an `XMLHttpRequest` from the SPA that meets a stale
session gets a 302 to Discord, which it can do nothing sensible with. Requests
the SPA makes should therefore be answered `401` with somewhere to log in,
while the browser's own navigation keeps the redirect that makes the old flow
work.

## What the old callback does with the token

The old flow is built for a browser that reloads pages, and it shows:

- the session cookie holds `user: {id}` and, while a login is in flight,
  `discord_oauth2: {state, continue}` — the `continue` is where to go afterwards;
- `/callback/discord` compares the returned `state` with the session's, exchanges
  the code, stores the Discord authorization, and issues a VC API token with
  `oauth2.register`, `vc.pay` and `vc.claim`;
- **the token comes back in response headers** (`x-access-token`, `x-redirect-to`,
  `x-expires-in`) and in a rendered page, and the session is renewed. `POST /token`
  re-issues one from the session later.

An SPA wants the same three steps with JSON: a login URL to send the browser to, a
callback that answers JSON (or redirects and leaves the token to `/token`), and
`POST /token` for renewals. The cookie session stays — it is what the consent
screen needs — but the token no longer has to travel in headers.

## The requirements, in the order they matter

1. **The consent screen** (`/oauth2/authorize`). It is what blocks the rest of
   OAuth2: no application can obtain a token without it, and it is the one page
   that must work when JavaScript does not, because OAuth2 sends browsers to it.
2. **Discord login and logout**, with the `continue` return path and the CSRF
   state check the old callback already does.
3. **The application list and detail** — the shapes already exist:
   `GET /oauth2/clients/@me` answers the list, and the detail is the same object.
   Editing is `PATCH /oauth2/clients/@me`, which the application edit page needs.
4. **The connect flow** (`/applications/:id/connect`) — `verify` and `change` are
   events in the LiveView and their requirements are **not yet read**.
5. **The contract approval** (`/contract/:id`) — likewise not read.
6. **The landing page**, which is prose plus a login button.
7. **The documents** — link them rather than rebuild them; they are static prose
   that already lives in `virtualcrypto-docs`.

## What the SPA will need that does not exist yet

- `GET /users/@me` and `GET /users/@me/balances` — the frontend's own endpoints.
  The first is implemented; **the second is not**, and its goldens are captured.
- A JSON login callback, since the old one answers with headers and HTML.

## Still to read before the last two requirements

`lib/virtualCrypto_web/live/connect_application.ex` (146 lines, and the only
LiveView with real events) and
`lib/virtualCrypto_web/live/contract/approve_application.ex`. Together they hold
the connect and contract flows, which are the only parts of the old site whose
behaviour is not described above.

## How it is served

`web/dist` is the router's **fallback**, not a mount: `/api` and `/health` keep
their own routes and only what nothing else claims is treated as a client-side
route. That order is the whole risk of serving a SPA from an API's own origin,
and `tests/web.rs` pins it — an asset is served as it is, an unknown path answers
with `index.html`, and `/health` still answers JSON.

`WEB_ROOT` says where the built assets are (`web/dist` by default), because a
deployment unpacks them somewhere else. Two things follow: **the frontend has to
be built before the server is started**, and nothing yet gives the hashed assets
the immutable caching they are built for — they are served without a
`Cache-Control`, which is correct but wasteful.

## What the callback needs that is not there, verified

Two gaps, both found by looking rather than assuming:

- ~~`DiscordAuth.insert_user` does not exist here.~~ Written: `vc_core::user::insert_user`
  records the authorization and creates the account in one transaction. It
  composes `insert_if_not_exists` with an upsert rather than repeating either,
  and it is not yet covered by a test — `vc-core` has no test harness, so the
  callback's own test is where it will be exercised.
- **Token issuance exists only in the test support.** `vc_auth::jwt::sign` over
  a `Claims` of `sub`, `exp`, `iss`, `aud`, `jti`, `kind`, `scopes` and `typ` is
  how the tests mint tokens, and that builder lives in `tests/support/mod.rs`.
  Production has no equivalent, so the callback and `POST /token` have nothing to
  call — and when one exists, the test helper should call it rather than own it.

The scopes a browser login is issued are the old app's three:
`oauth2.register`, `vc.pay` and `vc.claim`, with `kind` of `user`. `Kind::App`
already exists for the client-credentials tokens the OAuth2 provider will need.

### The exact shape of a token, so it does not have to be rediscovered

Issuance is two steps, and the second is the one that is easy to miss: sign the
claims, **and** write the `jti` down. A token whose id is not recorded cannot be
revoked, and `Guardian.revoke/1` is a `DELETE` against exactly that row.

```sql
INSERT INTO user_access_tokens (user_id, token_id, expires, inserted_at, updated_at)
VALUES ($1, $2, $3, $4, $4)
```

and the claims, whose values the API verifier already expects:

```rust
Claims {
    sub:   user_id.to_string(),
    exp:   issued_at + 3600,
    iat:   Some(issued_at),
    nbf:   Some(issued_at),
    iss:   "virtualCrypto",          // vc_auth::ISSUER
    aud:   Some("virtualCrypto"),    // vc_auth::AUDIENCE
    jti:   uuid,                      // the same one that was inserted
    kind:  "user",
    scopes: [...],
    typ:   Some("access".to_string()),
}
```

The hour is Guardian's, not a choice, and `iat`/`nbf`/`typ` are all set — a
token missing any of them fails verification, which is how the test support
learned to build them.
