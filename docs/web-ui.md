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

## The session model, which the SPA has to replace

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
