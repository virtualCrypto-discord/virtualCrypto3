# Personal grants

`docs/issue.md` is the grant this service already had: an application asks a *guild* for
`vc.issue`, an administrator answers `/grant approve code:` in that guild, and the application is
handed a token that may issue from the guild's pool. Every sentence of it is guild-scoped — the
column is NOT NULL, the code is unique inside a guild, and the token's account is the
application's own.

This is the same ask with a *person* on the other end: the application wants to do something that
concerns one user's own account, that user is the one who decides, and the token it gets acts as
them for the scopes they agreed to. It is what the personal access token cannot be — a PAT is the
whole account, for as long as nobody revokes it, and it says nothing about which application holds
it.

**This is an addition.** The Elixir has no user-scoped grant, no `vc.pay`/`vc.claim`/`vc.read`
scope it could name, and no column either could live in. There is no golden and no case to port;
`crates/vc-api/tests/personal_grants.rs` is this tree's own.

## The ask

`POST /oauth2/clients/@me/grant-requests` — the endpoint an application already uses — with
`discord_id` where it used to be `guild_id`:

```json
{ "discord_id": "123456789012345678", "scopes": ["vc.read", "vc.pay"], "expires_in": 600 }
```

Exactly one of `guild_id` and `discord_id` is required, and which one is there says which kind of
grant is being asked for: a server grant (an administrator of that guild decides, `docs/issue.md`)
or a personal grant (that user decides, this document). Both are still one pending ask per
application and target, both answer `{device_code, user_code, verification_uri, expires_in}`, and
both are polled the same way.

The scopes an application may ask a person for are the three a person's own account has to give:

| Scope | What it lets the holder do, as that user |
|---|---|
| `vc.read` | read the account: `/api/v2/users/@me` and its balances, claims, transactions and contracts, and a contract it is named in |
| `vc.pay` | spend from it: `POST /api/v2/users/@me/transactions`, and the bulk one |
| `vc.claim` | act on its claims: create one, approve, deny, cancel, set metadata |

Three things are deliberately **not** grantable, and each has its own reason rather than being an
omission:

- **`oauth2.register`** — registering and editing applications is administration *of the account*,
  not an operation *on its money*. A delegate is a list of things an application may do; who I am
  is not one of them. That is the personal access token's job, and it stays that way.
- **`vc.issue`** — a guild's pool is a guild's, and there is no person to ask.
- **`vc.contract`** — that is an application's own scope: it says the application may *ask* a user
  to lock money, and what makes it spend is each contract's own approval. A personal grant is not
  that side of the relationship.

## The decision

Two commands, because there are two things to decide and they are decided by different people.
`/grant server …` is what exists today — an administrator of the guild the ask named answers it,
reads the guild's authorized applications, and revokes one. `/grant user …` is this document: the
code, the list of what this account has approved, and the button that takes one back.

```
/grant user approve code:4f2a9c11
/grant user list
```

**Where the command is run does not matter.** There is no DM-only rule and no guild requirement:
what makes the decision the right one is that the caller *is* the user the ask named, so the same
press by anybody else decides nothing and is answered as a code that is not theirs. A guild channel
therefore costs nothing but the code being visible to whoever is reading — and the code is the
half a person chose to show, which is how the server grant's has always worked as well.

`/grant user list` reads the applications this account has approved, with the day each grant was
written and a button that revokes one. Revoking deletes the grant row, and every token issued for
it goes with it — the same cascade `/grant server revoke` has, for the same reason.

## The token

Exactly the shape the server grant's token has, because it is the same machinery:

| | |
|---|---|
| On the wire | the UUID of an `access_tokens` row — not a JWT |
| Lifetime | one hour, with a refresh token (180 days) from the same poll |
| Resolves to | the granting user's account, the application that asked, and the grant's scopes read at use |
| Revocation | the grant row deleted — the token row is a child of it — or the token deleted on its own |

Resolving to the *user's* account is the whole of what makes it a delegation: an application
holding it is that user for the endpoints it is accepted on, and for the scopes it carries.
Because the scopes are read from `grant_scopes` when the token is used rather than carried in it,
taking a scope away takes it away from tokens already issued.

**Where it is accepted.** Only the endpoints that take the extractor this adds: the v2 reads, the
transactions, and the claims. Everything else — registration, the consent screen, a contract's
party half, the guild issuing endpoint — refuses it as a token it does not know, because those
handlers take the JWT extractor. A personal grant therefore cannot reach `oauth2.register` by
construction rather than by a check that could be forgotten.

## Reads, and why they need a scope here

A session token and a PAT read `/api/v2/users/@me*` without a scope, and they keep doing that: the
token *is* the account, and asking it to prove it may look at itself would be ceremony. A grant is
not that — it is a list of what somebody else may do, and reading is something somebody else may
do. So the list is exhaustive: a grant token reads only with `vc.read`, spends only with `vc.pay`,
touches claims only with `vc.claim`. Two rules, then, and they are about two different things: a
credential that *is* the account, and a credential that acts *for* it.

The same reading applies to the party half of contracts — approving, refusing, withdrawing. Those
are a person's own decisions about their own money, taken in their own name; a delegation does not
take them. `/users/@me/contracts` is a read like any other, and it needs `vc.read`.

## Where the pieces go

- `crates/vc-core/migrations/0013_personal_grants.sql` — `grants.discord_id` and
  `grant_requests.discord_id` (with `guild_id` no longer NOT NULL, exactly one of the two set,
  `vc.read`/`vc.pay`/`vc.claim` added to the scope enum), and the indexes that follow from the new
  target: one pending ask per target, one pending code per (discord_id, code), and the grant upsert.
- `crates/vc-core/src/grant.rs` — the target becomes a value (`guild` or `user`) in the ask, the
  grant, the decision, the resolution and the notification, rather than a `guild_id` threaded
  through all of them.
- `crates/vc-api/src/routes/grant_requests.rs` — `discord_id` accepted, exactly one target.
- `crates/vc-api/src/command/grant.rs` — split into the two groups: `server` is today's command,
  `user` is the code, the list and the revoke, run wherever the caller happens to be.
- `crates/vc-api/src/notification.rs` — the decision is sent for both targets, and event `3`'s body
  carries which one.
- `crates/vc-api/src/routes/personal_token.rs` (name to be settled in review) — the extractor,
  which accepts a grant's token and yields what `Limited` yields, so no handler body changes.
- `crates/vc-api/src/routes/v2/{users,claims,transactions,contracts}.rs` — the reads, and the two
  write families, take that extractor instead of `Limited`.
- `docs/issue.md` gains a pointer here, `docs/oauth2.md`'s grant-request paragraph says which
  target it names, and `docs/qa.md`'s surface tables gain the new rows.

## The event

A decision about a personal grant is delivered to the application over its webhook, the way a
server grant's is — event type `3`, sent when the grant is written and again when one is revoked,
with the scopes as they stand.

The body carries which target was decided, and exactly one of the two is ever set:

```json
{"type": 3, "data": {"guild_id": null, "discord_id": "123456789012345678",
                     "scopes": ["vc.read", "vc.pay"]}}
```

`discord_id` is the person who decided, in the same shape the ask names them, and `guild_id` stays
the string it was: a body with one of them is the event, and "exactly one" is the ask's rule rather
than a second one. Nothing has been released with the old body, so this is a shape to change now
rather than a compatibility shim to keep — and the ping is the application's to answer with the
same poll it was already making.

## What is not here

- **A token kind on the wire.** It is a UUID row, like the server grant's.
- **Personal grants for anything but a person's own account.** There is no "grant for a guild I
  administer" here: that is the server grant.
- **A scope that means "act entirely as me".** That is the PAT, and it is a credential handed to a
  person rather than a permission an application asked for.
