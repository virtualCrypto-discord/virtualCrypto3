# Authentication and authorization model

Authorization combines **credential type, operation permission, relationship to the
object, and currency restriction**. An account id alone never proves the caller has
the account's own authority. Personal delegations are not converted into user JWTs.

## Credential types

| Credential | Representation | Account or resource it acts on |
|---|---|---|
| User JWT, including sessions and PATs | `Principal::Own`, `kind: user` | That user's account |
| Application JWT | `Principal::Own`, `kind: app` | The application's own account |
| Personal grant UUID token | `Principal::Delegated` | The approving user's account, with explicit operation and currency restrictions |
| Guild grant UUID token | Separate `GuildToken` extractor | The approving guild's issuing pool |

`Own` does not mean unrestricted authority. Existing scope, ownership and state
checks still apply. An application's JWT does not grant access to its owner's
personal assets. A personal grant never becomes an own credential, and personal
and guild grants are not interchangeable merely because both tokens are UUIDs.

## Personal scope catalogue

The `vc.delegate.` namespace is exclusively for personal delegations. Names are
matched exactly; there are no wildcards, prefix grants, aliases or implied scopes.
The shared catalogue is [delegation.rs](../crates/vc-core/src/delegation.rs).

| Scope | Operation |
|---|---|
| `vc.delegate.profile.read` | Read `/api/v2/users/@me` |
| `vc.delegate.balances.read` | Read the account's balances |
| `vc.delegate.claims.read` | Read claim lists and individual claims |
| `vc.delegate.contracts.read` | Read the user's contract list and individual contracts |
| `vc.delegate.contracts.payments.read` | Read a contract's payment history |
| `vc.delegate.payments.create` | Make single or bulk payments from the account |
| `vc.delegate.claims.create` | Create claims, including their initial metadata |
| `vc.delegate.claims.approve` | Approve a claim and pay it from the account |
| `vc.delegate.claims.deny` | Deny a claim addressed to the account |
| `vc.delegate.claims.cancel` | Cancel a claim made by the account |
| `vc.delegate.claims.metadata.write` | Set or delete the account's metadata on an existing claim |

Claim creation does not authorize claim approval. Payment creation does not
authorize claim approval, and approval does not authorize arbitrary payments.
Reading contracts does not imply reading their payment history. No read scope
implies a write, and no write scope implies access to a read endpoint. An authorized
write may still return its result, as the API already does.

A claim PATCH containing both a status transition and an explicit `metadata` field
requires **both** the transition's scope and `claims.metadata.write`, including when
metadata is `null`. All required permissions are checked before any mutation. With
metadata omitted, a transition does not change existing metadata values; the old
empty-object merge behavior is retained. Initial metadata belongs to claim creation
and does not require permission to edit existing claims.

No personal scope authorizes contract approval, refusal or withdrawal, application
registration/administration, or application-side contract creation/charging. These
require an own credential of the appropriate kind even if a delegation has every scope.

## Legacy v2 compatibility

**Existing v2 JWTs do not need any `vc.delegate.*` scope.** Their format and scope
meanings remain unchanged, for both `kind: user` and `kind: app`.

| Operation | Own user JWT | Own application JWT |
|---|---|---|
| Profile, balances, user's contract list, contract detail/history | No additional scope | No additional scope |
| Claim reads and writes | `vc.claim` | `vc.claim` |
| Single/bulk payments | `vc.pay` | `vc.pay` |
| Application contract list/create/charge | Denied | `vc.contract` |
| Contract approval/refusal/withdrawal | No additional scope | Denied |

Valid authentication and the object checks below are always required. Own
credentials do not acquire a grant's currency restriction. `vc.delegate.*` names
in a JWT do not imply legacy `vc.pay` or `vc.claim` permissions.

Conversely, personal grant requests reject `vc.read`, `vc.pay`, `vc.claim` and
wildcards as `400 invalid_scope`. Old broad personal-grant rows are not expanded
into the new scopes and do not authorize delegated operations. New explicit
permissions require a new approval. The migration adds enum values only; it does
not grant permissions or reinterpret existing approvals.

Guild issuing remains separate: a guild grant needs `vc.issue` and must cover the
guild's currency. None of the personal scope names authorizes guild issuing.

## Object, currency and state checks

An operation permission is necessary but not sufficient:

- The account must have the required relationship to the object: claimant, payer,
  contract party or contract-creating application, depending on the operation.
- For a delegation, the currency must be covered by its grant.
- Domain rules still enforce balances, pending/active states, expiry and other
  conditions. An approval scope does not bypass a claim's payer check.

The order of these checks is endpoint-specific. Contract details and histories
check the relationship first, returning 404 to unrelated callers before applying
the currency check.

Currencies are stored as ids, not units or URLs. Empty means all currencies. A
restricted grant retains deleted currency ids so deletion cannot turn its last
restriction into unrestricted access. See [resources.md](resources.md).

| List/read | Currency restriction |
|---|---|
| Balances | Return only covered rows |
| Claims | Filter after fetching a page; derive continuation from the unfiltered page |
| Contracts | Filter in SQL before applying cursor pagination and limit |
| Claim detail, contract detail/history | Reject an excluded currency with 403 |

A filtered claim page may be empty and still have a next-page `Link`. Excluded
contracts do not consume the contract page's limit. An application account has no
Discord user and therefore has an empty `/users/@me/contracts` list; its own
created contracts are listed through `/contracts`.

The `resource` set applies to every granted currency-specific operation. It does
not express a different currency set per scope. Profile access has no currency
target and is controlled by its own scope.

## Enforcement boundaries

Account handlers use `Authorized<P>` from
[limited.rs](../crates/vc-api/src/routes/limited.rs). Permission types explicitly
name profile, balance, claim, contract or history reads; payment creation; claim
creation; application contract operations; or own-user contract decisions.
There is no default permission and no conversion to `AuthUser`. The sealed
permission catalogue centralizes the own/delegated policy.

Claim PATCH uses a separate `ClaimPatch` body extractor. It authenticates the
caller, parses the body, determines the exact transition and whether metadata is
present, then verifies every required scope. A handler cannot extract the broad
PATCH family on its own and forget to check which mutation was requested. Invalid
status/body semantics continue to be handled by the endpoint; they do not acquire
permission to perform a different transition.

**The types enforce operation permission, not every object constraint.** Handlers
and domain code must still enforce relationships, currencies and state. New
endpoints also need individual-target checks and correct list pagination.

Delegated scopes and resources are read from the grant on each authentication.
Each device approval has an independent scope/resource pair and token family.
New approvals cannot change existing tokens or combine their permissions. Revoking
one grant stops its tokens from authenticating while other grants remain valid.
This does not retroactively cancel a request that already passed authorization.
Account APIs retain their account-based rate limit.

## Refusals and public endpoints

- Missing personal scope or excluded currency: 403 `insufficient_scope`.
- Credential kind cannot perform the operation: 403 `invalid_token` with the
  existing `permission_denied` description.
- Invalid personal token, or a guild token at an account endpoint: 401 `invalid_token`.
- Object relationship or state refusals retain each endpoint's existing responses.

Currency metadata remains public. `GET /currencies/{id}` checks an optional valid
grant's resources, but anonymous access and the query-form lookup remain available;
that check is not a confidentiality boundary. Account balances, claims and contracts
are protected separately. Discord commands authenticate signed interactions through
a separate path. `/grant approve` reviews and confirms either target kind; `/grant user` and
`/grant server` manage grants. See [personal-grants.md](personal-grants.md) for
identity checks, globally unique pending codes and request-bound confirmation.

## Regression coverage

[grant_resources.rs](../crates/vc-api/tests/grant_resources.rs) tests every delegated
scope against read and write operations, compound PATCH permissions and unchanged
state after denial, legacy JWT compatibility, excluded currencies and contract
pagination. [grant_requests.rs](../crates/vc-api/tests/grant_requests.rs) exercises
request, approval and token polling with the explicit scope names.
