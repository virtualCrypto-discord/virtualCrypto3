# Personal access tokens

An application's registration is a *user* action: `GET`/`POST /oauth2/clients`,
`POST /applications/{id}/connect` and `GET`/`DELETE /applications/{id}/grants` all take a
`kind: user` token and `oauth2.register`. The only way to get such a token today is `POST /token`
with a live browser session cookie, which means the person doing the setting up has to be sitting
in a browser — and anything that is not a browser (an agent, a script, a terminal) cannot do it at
all, because the session cannot be handed over and the token it produces lives for an hour.

A personal access token is that same token with a longer life and a name. Nothing about what it
*can* do is new: it is a `kind: user` token carrying the browser session's scopes, so every
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
| Lifetime | 365 days |
| Storage | a `user_access_tokens` row, with `name` set — which is what tells a PAT apart from the hour-long row a session writes |
| Revocation | deleting that row, which is how every JWT here is revoked: the signature stays valid, the `jti` stops resolving |

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
| `/pat create name:` | an ephemeral message holding the token, once, with the day it expires and the scopes it carries |
| `/pat list` | the names, each with the day it expires, and never a token — a list is for recognising a credential, not for reading it back |
| `/pat revoke name:` | the name is forgotten and the token is dead from that moment, because the row it resolves against is gone |

The name is required, unique per account (`user_access_tokens (user_id, name) WHERE name IS NOT NULL`),
and 1〜32 characters. Unique because revocation names it — two tokens called `laptop` could not be
told apart — and required because an unnamed list is a list of one indistinguishable token per row.

Ephemeral is not decoration: Discord keeps an ephemeral response to the interaction that asked for
it, so the token is visible to the person who typed the command and to nobody in the channel. It is
the one place this service puts a secret in a message, and it is the point of the command.

## What it is for

Handing an account to something that is not a person and is not a browser: an agent registering an
application, a script renewing a grant, a local tool that reads a balance. It is not for other
people, and it is not a delegation — there is no "acts on behalf of" anywhere in it, because the
token *is* the account.

The LLM use case is why it exists: registration and connect are the two operations that are tedious
to do by hand in Discord, and they are exactly the two a `kind: user` token with `oauth2.register`
unlocks. Whoever holds the token speaks as the account, so it belongs in the same place as a
password: shown once when it is made, revoked when it is not needed, never pasted anywhere that
someone else can read it.

## What is deliberately not here

- **Scope selection at mint time.** One scope set, the browser's. A selection menu is a screen to
  read every time, for a choice whose alternatives (say, a token that may not pay) have no caller
  yet.
- **A web page listing tokens.** The site has no authenticated surface, and a management operation
  that only a browser can reach is the thing this service does not do.
- **Reading the token back.** A PAT is shown once, at the interaction that made it. `/pat list`
  cannot answer with it, because the row does not store it — the JWT is signed, not kept, and the
  only copy is the one Discord delivered.
- **A token that outlives revocation, or survives a purge.** The row's `expires` is the PAT's own
  365 days, so the purge job that already removes expired `user_access_tokens` rows removes this
  one too, and the token dies at the same moment.
