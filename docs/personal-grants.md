# Personal grants

A personal grant lets an application perform explicitly approved operations on a
user's account. It is separate from a PAT, which is the account's own credential,
and from a guild grant, which authorizes issuing from a guild's pool.

This is an addition to v2. Existing application and user JWTs retain their legacy
`vc.pay` and `vc.claim` semantics. Personal grants use only the fine-grained
`vc.delegate.*` namespace described in [authorization.md](authorization.md).

## Request

An application's own token with `oauth2.register` calls
`POST /oauth2/clients/@me/grant-requests`:

```json
{
  "discord_id": "123456789012345678",
  "scopes": ["vc.delegate.balances.read", "vc.delegate.payments.create"],
  "resource": ["https://vcrypto.sumidora.com/api/v2/currencies/12"],
  "expires_in": 600
}
```

Exactly one of `discord_id` and `guild_id` is required. `discord_id` selects a
personal request; `guild_id` selects the guild issuing flow. The result contains
`device_code`, `user_code`, `verification_uri` and `expires_in`. There is one
pending request per application and target. Device polling and token refresh use
the existing grant machinery.

For a personal request, each scope must be an exact member of this catalogue:

| Scope | Permission |
|---|---|
| `vc.delegate.profile.read` | Read the user's profile |
| `vc.delegate.balances.read` | Read balances |
| `vc.delegate.claims.read` | Read claim lists and details |
| `vc.delegate.contracts.read` | Read contract lists and details |
| `vc.delegate.contracts.payments.read` | Read contract payment histories |
| `vc.delegate.payments.create` | Create single or bulk payments |
| `vc.delegate.claims.create` | Create a claim, with optional initial metadata |
| `vc.delegate.claims.approve` | Approve and pay a claim |
| `vc.delegate.claims.deny` | Deny an incoming claim |
| `vc.delegate.claims.cancel` | Cancel an outgoing claim |
| `vc.delegate.claims.metadata.write` | Set or delete metadata on an existing claim |

No scope implies another. In particular, a claim creator cannot approve a claim,
and a payment creator cannot approve claims. A status change with explicit metadata
requires both the status operation's permission and metadata-write permission.
An empty scope list grants no account operation. Duplicate or unknown names,
`vc.delegate.*` wildcards, and the old broad `vc.read`, `vc.pay`, `vc.claim` names
are `400 invalid_scope`; none is an alias for a set of new permissions.

The API catalogue is shared with authorization through
[`vc_core::delegation::Scope`](../crates/vc-core/src/delegation.rs).

## Currency restrictions

RFC 8707's `resource` selects currencies independently of operations:

- `https://<site>/api/v2/currencies/{id}` selects one currency.
- `https://<site>/api/v2/currencies` selects all currencies.
- Omitting resources has the same meaning as selecting all currencies.

The same resource set applies to every currency-specific operation in the grant.
A nonabsolute URI, a foreign service URI or an invalid currency is
`400 invalid_target`. Stored currency ids survive a site move. Deleted currency
ids are retained so deleting a grant's last currency cannot widen it to all
currencies. See [resources.md](resources.md).

## Discord approval and management

There is one approval command and two management commands:

- `/grant approve code:<user_code>` reviews either a personal or a server request.
  It displays the application name/client id, target, exact scope names with their
  meanings, and the currencies. Only the confirmation button writes the grant.
- `/grant user` lists applications with access to the caller's account, two per
  page, with their scopes, currencies and revoke buttons. It works in servers
  and DMs and does not require server administrator permissions.
- `/grant server` lists applications allowed to issue in the current server, five
  per page. It requires administrator permission in that server.

Personal requests can be reviewed and approved only by the requested user.
Server requests require an administrator in the requested server; they cannot
be approved from a DM or another server. Confirmation, pagination and revocation
buttons recheck the interaction actor. Personal list buttons are bound to that
user, and a copied button cannot act on someone else's grants.

Pending user codes are **globally unique**, across all users and servers. Migration
`0018_global_grant_codes.sql` replaces the target-local unique index. The generator
uses random UUID bytes, and a collision retries the transaction with a new code.
No disambiguation screen or target guessing is needed. Expired pending rows also
reserve their codes until they are removed.

A confirmation button contains the request id, not its reusable user code or its
secret device code. A deleted/expired/replaced request cannot be approved through
an old button. The decision checks pending status and expiry inside the transaction;
concurrent confirmations can write the grant only once and notify only once.

A personal approval creates the user's account if needed and replaces the existing
grant's exact scopes and currencies. Old spending scopes cannot survive a new
read-only approval and inherit its currencies. Issued tokens read the updated grant.
The review screen explicitly states this replacement behavior.

Revoking a personal grant deletes it and its dependent access/refresh tokens.
Guild revocation retains the existing behavior of removing `vc.issue` from the
grant. Old `/grant list` interactions remain a server-list alias, but the registered
commands are `approve`, `user`, and `server`.

Personal approval and revocation notify the application using webhook event type 3:

```json
{"type":3,"data":{"guild_id":null,"discord_id":"123456789012345678",
                  "scopes":["vc.delegate.balances.read"]}}
```

Revocation sends an empty scope list. The existing guild event shape is unchanged.

## Token and enforcement

| Property | Behavior |
|---|---|
| Wire form | UUID of an `access_tokens` row, not a JWT |
| Access-token lifetime | One hour |
| Refresh-token lifetime | 180 days |
| Account | The approving user's account |
| Authority | A personal delegation, never `AuthUser` or an application's own credential |
| Scopes/resources | Read from the grant when the token is authenticated |
| Revocation | Deleting the grant removes its dependent tokens; a token can also be deleted independently |

Personal grants cannot authorize application administration, guild issuing,
application-side contract creation/charging, or a user's contract approval,
refusal or withdrawal. Adding scopes cannot change the credential's kind.

The operation policy is enforced by typed extractors before domain work. Target
relationship, currency and state checks remain necessary. Out-of-resource
operations are `403 insufficient_scope`; lists return only covered data.
The precise endpoint policy, compound PATCH rules and pagination behavior are
in [authorization.md](authorization.md).

## Compatibility and schema

Migration `0017_delegation_scopes.sql` adds the explicit personal scope names.
It does not reinterpret existing JWTs or convert old broad personal permissions.
An old personal grant containing only `vc.read`, `vc.pay` or `vc.claim` has no
new delegated permission. Applications must obtain a new explicit approval;
old v2 own-account JWTs continue working without a new scope or approval.

The personal target columns were introduced in migration `0013`; resources were
introduced in `0014`, with deletion-safe restrictions in `0015`. Scope names on
the wire are strings, and grant request/poll/refresh payload shapes are unchanged.
