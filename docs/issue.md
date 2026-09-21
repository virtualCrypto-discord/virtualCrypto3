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
was: the code flow has always answered with an `access_tokens` row's own id.
`Authz.md` calls `guild` a kind of access token; here the kind is what the
token resolves to, and `vc_core::grant::resolve_token` is what resolves it —
the `access_tokens` row, its grant, and the grant's scopes, refused when the
row is expired, revoked, or issued for a grant with no guild.

It comes from the same places it always has:

- the device poll below, once the guild approved the ask;
- `grant_type=authorization_code`, once the consent screen was approved with
  `scope=vc.issue`.

A guild token already issued keeps working until it expires, and it reads the
grant's scopes when it is used rather than carrying them — so taking a scope away
takes issuing away with it.

## How a guild says yes — two ways in, one ask

A guild token is what a grant makes spendable, and a grant with `vc.issue` is
what the guild says yes to. The permission always starts with the application
asking — `POST /oauth2/clients/@me/grant-requests {"guild_id": "...",
"scopes": ["vc.issue"]}`, with the registration token — and the ask is what the
approval answers. The grant is written from the ask's own scopes, never from
anything the approver names: an approval that granted something else would be a
permission nobody asked for.

The 201 answers the device flow's four values: `device_code` (what the poll
names), `user_code` (what the administrator types, eight characters), and
`expires_in` (how long the ask lives, ten minutes unless asked shorter).
`verification_uri` is `"discord"`: there is no URI to open, because the
approval happens in the guild, where the administrator types the code the
application put in front of them.

### 1. The application's own page

The application's owner reads `/applications/{client_id}/grants`: the guilds
the application may issue in — the guild id, the name Discord knows it by
(`null` when Discord cannot be asked), the scopes, and the time of the
decision — and takes one back with
`DELETE /applications/{client_id}/grants/{guild_id}`. The grant stays and only
its scope goes, so a guild token already issued stops issuing and nothing else
changes. A `client_id` that is not the caller's own is a 404, so this cannot be
used to ask which client ids are real. Writing is not here on purpose: a grant
is the guild's decision, and a user token calling this endpoint is the
application's owner, not the guild.

### 2. Discord: `/grant`

For the guild that never opens a browser — and the only way a guild that is not
the application's owner says yes. `/grant` is guild-only and asks the
administrator bit, like `/issue` beside it:

- `/grant list` shows the applications this guild has allowed to issue, five to a
  page, each with the button that takes the permission back. Asks are not here:
  they are the application's own business, and
  `GET /oauth2/clients/@me/grant-requests` is where the application reads them.
- `/grant approve code:<user_code>` approves an ask and writes the grant from
  its scopes. A code that names nothing pending here is refused the same way
  whether it never existed, belongs to another guild, or already expired.
- The list's revoke button takes a permission back by the application it belongs
  to: the issuing scope goes and the grant row stays, which is what a guild token
  already issued is read from — it stops issuing without being told. The owner's
  page has the same taking-back behind
  `DELETE /applications/{client_id}/grants/{guild_id}`.

There is no refusal anywhere in this command on purpose: an approval is the
only decision, and an ask that is never approved simply stays pending until it
expires.

## How the device gets its token — the poll

`POST /oauth2/token` with
`grant_type=urn:ietf:params:oauth:grant-type:device_code` and the `device_code`,
authenticated by Basic with the application's own id and secret:

- still pending: `400 {"error":"authorization_pending"}` — keep polling;
- approved: `200` with the guild token, minted from the grant the approval
  wrote;
- unknown, expired, or approved-but-revoked: `400 {"error":"invalid_grant"}` —
  ask again.

A guild token already issued keeps working until it expires, and it reads the
grant's scopes when it is used rather than carrying them — so taking a scope away
takes issuing away with it.

## The push next to the poll

An application that named a `webhook_url` at registration does not have to
poll blind: the approval also delivers a type-3 event —
`{"type": 3, "data": {"guild_id": "...", "scopes": [...]}}` — signed the way
every delivery is signed, with the application's own key. The token still comes
from the poll; the push is the ping that says to poll now, the way CIBA's ping
carries the `auth_req_id` and the tokens come from the token endpoint. An
application without a webhook polls instead, which is why the push is a ping
and not the decision itself.

A revoke travels as the same event: the `scopes` are what the grant has left,
empty once the issuing scope is the one taken away. That is the one thing the
poll's answer does not carry — it names no scopes — so the ping is what tells a
device what its token may still do. Un-asking a pending code is not a decision
and pings nothing: the ask minted no token, and the poll already answers that it
is gone.

Which events reach the webhook is the application's own choice, named at
registration and changed with an edit: `subscribed_events` names the `type`
values it wants — `2` for a claim update, `3` for a grant decision, `4` for a
contract decision (`docs/contracts.md`) — and it is
exactly what it says: checked is sent, unchecked is not, and empty is nothing.
An application that only issues names `[3]` and is never woken for a claim; one
that only pays names `[2]` and never for a grant. The handshake's `1` is not
subscribable, because a PING is a check rather than an event.

## The application's own view

`GET /oauth2/clients/@me/grant-requests` reads back what the application asked
— the codes, the guild, the scopes, the status — which is what a device shows
its operator next to the code to type. A user token cannot ask a guild for a
permission their application holds: the caller has to *be* the application the
grant would be written for.
