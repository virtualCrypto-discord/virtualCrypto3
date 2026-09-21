# Personal access tokens

An application's registration is a *user* action: `GET`/`POST /oauth2/clients`,
`POST /applications/{id}/connect` and `GET`/`DELETE /applications/{id}/grants` all take a
`kind: user` token and `oauth2.register`. The only way to get such a token today is `POST /token`
with a live browser session cookie, which means the person doing the setting up has to be sitting
in a browser — and anything that is not a browser (an agent, a script, a terminal) cannot do it at
all, because the session cannot be handed over and the token it produces lives for an hour.

A personal access token is that same token with a name and no end but revocation. Nothing about
what it *can* do is new: it is a `kind: user` token carrying the browser session's scopes, so every
endpoint that accepts a session token accepts it, and the API reference needs no fourth kind of
caller.

**This is an addition.** The Elixir has no personal access token, no API key, and no column that
could hold one (`docs/known-gaps.md`). There is therefore no golden and no Elixir test to port;
`crates/vc-api/tests/pat.rs` is this tree's own.

## What it is

| | |
|---|---|
| Value | a JWT, the same HS512 shape as every other token this service issues |
| `kind` | `user` — a PAT is the account it belongs to, with no authority of its own |
| Scopes | `oauth2.register`, `vc.pay`, `vc.claim` — the scopes a browser session's token carries, from one shared constant, so the two cannot drift apart |
| Lifetime | none — the claims carry no `exp`, so no clock refuses it and only revocation ends it |
| Storage | a `user_access_tokens` row, with `name` set and `expires` NULL — which is what tells a PAT apart from the hour-long row a session writes |
| Revocation | deleting that row, which is how every JWT here is revoked: the signature stays valid, the `jti` stops resolving |

The lifetime is the one place a PAT is not a session token with a name, and the reason is the
caller: a browser re-makes its token because it holds the session that can, and the thing this
token exists for — an agent, a test, a script in a crontab — has nobody to notice it stopped
working. A year, then a re-make by hand, is that same chore spread out. So the token has no day it
dies on, the row has no date the purge job can delete it by, and `/pat revoke` is the whole of what
ends it. What that costs is the count: nothing expires, so nothing thins the list, which is what
[the cap](#how-many) is for.

The scopes are the whole of what a browser session can do, deliberately: a token handed to a tool
is the account with a longer memory, not a new kind of authority. Narrowing it (an `oauth2.register`-only
token, a token that may not pay) is a real thing to want, and it is **not** this: it would need a
choice at mint time and a way to say "not this scope" that a person can read back later.

## Where it is made

`/pat`, in Discord, with three subcommands. Every other management operation in this service is
completable from Discord alone, and a credential that could only be minted by opening the web UI
would be the first exception to that — the site has no authenticated page at all, on purpose
(`docs/web-ui.md`).

| Command | Answer |
|---|---|
| `/pat create name:` | an ephemeral message holding the token, once, with the scopes it carries and the fact that it has no expiry |
| `/pat list` | the names, and never a token — a list is for recognising a credential, not for reading it back, and with no day to show there is nothing else it could say |
| `/pat revoke name:` | the name is forgotten and the token is dead from that moment, because the row it resolves against is gone |

The name is required, unique per account (`user_access_tokens (user_id, name) WHERE name IS NOT NULL`),
and 1〜32 characters. Unique because revocation names it — two tokens called `laptop` could not be
told apart — and required because an unnamed list is a list of one indistinguishable token per row.

### How many

Twenty-five per account. The list is one message and nothing pages it, so there has to be a number
past which it would not fit — and a screen that paged a list of credentials nobody browses is a
screen with an arrow nobody presses. Twenty-five is well inside what a message holds and far past
what a person keeps track of; `/pat list` is where somebody at the limit finds the names to give
up, and `/pat revoke` is how they give one up.

Ephemeral is not decoration: Discord keeps an ephemeral response to the interaction that asked for
it, so the token is visible to the person who typed the command and to nobody in the channel. It is
the one place this service puts a secret in a message, and it is the point of the command.

## What it is for

Handing an account to something that is not a person and is not a browser — an agent registering an
application, a script reading a balance, a tool approving a contract it was named in. The list is
examples and not the whole of it: the token *is* the account, so what it can do is what the account
can, which is the browser session's surface. It is not for other people, and it is not a
delegation — there is no "acts on behalf of" anywhere in it, because the token is the account
rather than somebody speaking for it.

The LLM use case is why it exists: registration and connect are the operations that are tedious to
do by hand in Discord, and they are the ones a `kind: user` token with `oauth2.register` unlocks.
Whoever holds the token speaks as the account, so it belongs in the same place as a password:
shown once when it is made, revoked when it is not needed, never pasted anywhere that someone else
can read it.

What that surface is, in full, because "the browser's surface" is not a list: every endpoint that
takes a user token — the `/api/v2` family it is a caller of, the registration endpoints, the connect
and grant endpoints, and the party half of the contract family (`approve`, `refuse`, `withdraw`,
reading a contract it is named in, and its own list) — and `POST /oauth2/token/revoke`, which takes
any token, this one included. The application half of contracts — creating one, spending it — is not
in it and cannot be: those endpoints take an `app` token, which is a different caller, and an
application holds its own `client_id` and `client_secret` for that.

## What is deliberately not here

- **Scope selection at mint time.** One scope set, the browser's. A selection menu is a screen to
  read every time, for a choice whose alternatives (say, a token that may not pay) have no caller
  yet.
- **Paging.** The list is one message and shows every row, which is the reason the count is
  bounded instead: a page of names nobody asked for is a component that exists to be ignored.
- **A web page listing tokens.** The site has no authenticated surface, and a management operation
  that only a browser can reach is the thing this service does not do.
- **Reading the token back.** A PAT is shown once, at the interaction that made it. `/pat list`
  cannot answer with it, because the row does not store it — the JWT is signed, not kept, and the
  only copy is the one Discord delivered.
- **A day to expire on.** No `exp` claim, no `expires`, so the purge job that removes expired
  `user_access_tokens` rows never touches a PAT: what deletes it is `/pat revoke`, or the account
  it belongs to being deleted, which takes the row with it.
