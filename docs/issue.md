# Issuing from a guild's pool over the API, and the permission that allows it

`Authz.md` says a `guild` token is what may `give`, and `Rest.md` documents no
endpoint that does it. That half-sent sentence is now a contract:
**`POST /api/v2/currencies/issue`** issues from a pool, on the authority of a
guild token that carries the **`vc.issue`** scope.

This is **not a port**. The Elixir router has no issuing endpoint, and its
Guardian refuses any kind but `user` and `app` — so nothing below was read out
of it. `Command.md` in the official docs still documents `/give`; the command
this service registers is [/issue](#the-endpoint), and the endpoint below is
its API half.

## The endpoint

`POST /api/v2/currencies/issue`, with a guild token in `Authorization: Bearer`.

The guild is the token's, so the path names none: a guild token can only ever
spend the pool of the guild it was issued for. The body is one object:

```json
{ "receiver_discord_id": "408939071289688064", "amount": "100" }
```

Both values are numbers written as strings, the way this API carries numbers.

- The amount is **required**. The command answers an omitted amount with the whole
  pool; here the caller is a bot and a forgotten field must not drain a guild.
- Success is **201 `{"amount": "...", "pool_amount": "...", "unit": "..."}`** — what
  was issued, what the pool holds after it, and the currency's unit.
- `vc.issue` is required: a token without it is 403
  `{"error":"insufficient_scope","error_description":"token_verification_failed"}`.
- Anything that is not a guild token — a user or application token, an unknown
  token, an expired one, one whose grant has no guild in it — is 401
  `{"error":"invalid_token"}`.
- The three domain failures are the payment endpoint's, word for word:

| Failure | Answer |
| --- | --- |
| the guild has no currency | 400 `{"error":"invalid_request","error_info":"not_found_currency"}` |
| the amount is not a positive number | 400 `{"error":"invalid_request","error_description":"invalid_amount"}` |
| the pool holds less than the amount | 409 `{"error":"conflict","error_info":"not_enough_amount"}` |

- `Idempotency-Key` is honoured the way the payment endpoint honours it: the same
  key, the stored answer; a key still in flight, 409 `processing`. The key's
  namespace is the application's own account, because the application is the only
  identity a guild token has.

## The guild token

It is not a JWT, and that is where the Elixir was going rather than where it
was: the code flow and `client_credentials` with a `guild_id` have always
answered with an `access_tokens` row's own id. `Authz.md` calls `guild` a kind
of access token; here the kind is what the token resolves to, and
`vc_core::grant::resolve_token` is what resolves it — the `access_tokens` row,
its grant, and the grant's scopes, refused when the row is expired, revoked, or
issued for a grant with no guild.

It comes from the same places it always has:

- `grant_type=client_credentials` with basic auth and a `guild_id`, once the guild
  holds a grant for the application;
- `grant_type=authorization_code`, once the consent below was approved with
  `scope=vc.issue`.

A guild token already issued keeps working until it expires, and it reads the
grant's scopes when it is used rather than carrying them — so taking a scope away
takes issuing away with it.

## How a guild says yes — three ways in

A guild token is what a grant makes spendable, and a grant with `vc.issue` is
what the guild says yes to. There are three ways in, and they write the same
grant.

### 1. The consent screen

`/oauth2/authorize` already asks the guild and the caller whether they may act
for it — the owner, or a member whose roles carry the administrator bit — and
`scope=vc.issue` now passes its scope check alongside `openid`. Approving writes
the authorization code whose exchange writes the grant's scopes, and the grant is
`vc.issue`.

### 2. The application's own page

The application's owner manages `/applications/{client_id}/grants`:

- `GET` lists the guilds the application may issue in — the guild id, the name
  Discord knows it by (`null` when Discord cannot be asked), the scopes, and the
  time of the last decision.
- `POST {"guild_id": "..."}` asks the guild's permission the consent screen's
  way, and writes it — 201 with the guild and its scope.
- `DELETE /applications/{client_id}/grants/{guild_id}` takes the issuing
  permission back. The grant stays and only its scope goes, so a guild token
  already issued stops issuing and nothing else changes.

Adding a guild is the owner's act about a guild they may act for: the caller must
own the application **and** be its guild's owner or administrator. Revoking asks
nothing of the guild — an owner narrowing what their own application may do is
not a decision the guild has to be asked about. A `client_id` that is not the
caller's own is a 404, so this cannot be used to ask which client ids are real.
The application's own page in the SPA manages this: the detail page lists the
guilds, adds one by its id, and revokes with a button.

### 3. Discord: `/grant`

For the guild that never opens a browser. `/grant` is guild-only and asks the
administrator bit, like `/issue` beside it, and has two subcommands:

- `/grant allow <client_id>` shows what the application is — its name and the
  id — and a confirmation. Pressing it writes the grant with `vc.issue`.
- `/grant list` shows what the guild has been asked and not answered, from the
  application's ask below, each with **許可する** and **拒否する**. Pressing
  either decides the request and redraws the list.

The administrator is checked twice — when the command runs and when the button
is pressed — because the button outlives the message it came in.

## How an application asks — the request

`POST /oauth2/clients/@me/grant-requests {"guild_id": "..."}`, with the
application token registration answered with. It writes a pending row, which is
what `/grant list` shows the guild, and 201 answers with its id and `pending`.

An ask while one is pending is the same ask: it answers with the row that is
already there. A denied ask may be asked again, because the guild saying no ends
the ask rather than the conversation — and `GET` on the same path reads back
what was asked and what the guild said, which is what the application polls
before it exchanges a guild token.

A user token cannot ask a guild for a permission their application holds: the
caller has to *be* the application the grant would be written for.
