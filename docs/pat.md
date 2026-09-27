# Personal access tokens

A personal access token (PAT) lets a tool call the API as its owner. It is issued
from Discord with `/pat create name:`. Browser authorization does not issue
account tokens; comparing a PAT to a browser session token is obsolete.

## Authority and lifetime

| Property | Behavior |
| --- | --- |
| Value | HS512 JWT with `kind: user` |
| Scopes | `oauth2.register`, `vc.pay`, `vc.claim`; fixed at issuance, with no scope selection |
| Authority | Application registration and connection, balance access, payments, claims, and the party's contract operations, including approval |
| Lifetime | No `exp` claim and no database expiry; usable until revoked or its account is deleted |
| Storage | `user_access_tokens` records `user_id`, `token_id`, and a name; the JWT value is not stored |
| Revocation | Deletes the matching named token row belonging to the caller; subsequent API requests return 401 even though the JWT signature remains valid |

`vc_auth::issue::BROWSER_SCOPES` retains its historical name but now describes
the PAT scope set. Application operations such as creating and spending a
contract require an `app` token and are not granted by a PAT.

The holder can act as the account within these permissions, including spending
its balance. Creation and help explain this directly, together with the lack of
expiry and scope selection. Store a PAT as carefully as a password and revoke
it when it is no longer needed or has been exposed.

## Discord commands

Both commands work in servers and DMs. Every response is private to the person
who invoked it.

| Command | Result |
| --- | --- |
| `/pat create name:` | Shows the value once, to the invoker, with its authority, scopes, unlimited lifetime, and the path to revocation |
| `/pat list` | Shows names and a red 「失効」 button beside each name; never shows token values |

There is no separate `revoke` subcommand. Pressing 「失効」 immediately revokes
that token and refreshes the same private list with the result. Tools using it
can no longer authenticate. The list says this before the user presses a button.

Names contain 1–32 characters and must be unique within the account. A duplicate
or a twenty-sixth token is refused, with a link to `/pat list` to free a slot.
Revoked names can be reused. Names are escaped when rendered as Markdown.

The cap remains 25 tokens. The list displays 10 names per page, with ⏮️ (previous) and
⏭️ (next) buttons and the current page/count. A row uses a Section, Text Display,
and Button; pagination keeps even a full page with a result notice within
[Discord's 40-component limit](https://docs.discord.com/developers/components/reference).
If revocation empties the final page, the list returns to the preceding page.
Revoking the last token shows how to create one again.

Buttons carry the owner, page, and immutable token ID, never a token value or a
name. The handler checks the interaction user, and deletion also checks the
account and that the token is named. A copied button cannot revoke another
account's token. A stale button cannot revoke a replacement with the same name;
it reports that the old token is already gone and refreshes the list.

## Verification

`crates/vc-api/tests/pat.rs` covers issuance, authority, expiry absence, duplicate
names, empty and overlong names, a 32-character Japanese name, the 25-token cap,
pagination, private replies, and API rejection after button revocation. It also
covers other-account access, forged token IDs, unnamed tokens, repeated clicks,
name reuse, and obsolete commands. `interactions_management_ack.rs` checks that
revocation acknowledges the interaction before changing data, and that rejected
callbacks and replayed interactions do not repeat mutations.

For Discord device checks and evidence without token values, see
[`docs/qa.md`, UX-10](qa.md#ux-10-pat-recheck).
