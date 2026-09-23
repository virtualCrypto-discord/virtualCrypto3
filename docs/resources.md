# Currency restrictions — RFC 8707 `resource`

A grant says *what* an application may do — `vc.issue` for guilds, or
the individual `vc.delegate.*` scopes for personal grants — and nothing about *which currency*. An application allowed to pay is
allowed to pay any currency in the account, and one allowed to issue may issue
from the whole pool. That is a wider permission than most applications want and a
wider one than a person means to give when they read the screen.

This is the design for narrowing it: the client names the currencies it intends
to touch, the screen says so, and the token it gets is worth nothing against the
rest.

## The specification

RFC 8707, *Resource Indicators for OAuth 2.0*, is the parameter for exactly this.
Its `resource` names the protected resource a token is requested for:

> `resource` — Indicates the target service or resource to which access is being
> requested. Its value MUST be an absolute URI, as specified by Section 4.3 of
> RFC 3986. The URI MUST NOT include a fragment component.

and §2.2 gives the reading of a token that names both:

> The semantics of such a request are that the client is asking for a token with
> the requested scope that is usable at all the requested target services.
> Effectively, the requested access rights of the token are the cartesian product
> of all the scopes at all the target services.

§3 adds the part that makes a currency a resource rather than a stretch of one:

> Some servers may host user content or be multi-tenant. In order to avoid attacks
> where one tenant uses an access token to illegitimately access resources owned by
> a different tenant, it is important to use a specific resource URI including any
> portion of the URI that identifies the tenant, such as a path component.

A currency is that: one guild's, named in this API's own path. It is also why
RFC 8707 is the specification taken here and RFC 9396, *Rich Authorization
Requests*, is not. Rich authorization details could express "pay in nyan, read in
kaguya" as one object — `{"type": "vc.currency", "identifier": "nyan", "actions":
["pay"]}` — but the actions it would carry are the scopes this service already
has, so every grant would say the same thing twice: once as `vc.delegate.payments.create`, once as
`actions: ["pay"]`. RFC 8707 leaves the scopes to the scope parameter, which is
what they are for, and adds only the one thing missing.

## The resource identifier

An absolute URI, built from the site the API is served on and the currency's own
path:

```
https://vcrypto.sumidora.com/api/v2/currencies/12
```

`/api/v2/currencies/:id` is a real endpoint — the one that reads a currency — and
the id in it is the currency's own, so the identifier is a location rather than a
name that has to be looked up. Units are not used: a unit is unique inside a guild
and this URI has to identify a currency from anywhere.

The collection is the second form, and it is how a client asks for all of them:

```
https://vcrypto.sumidora.com/api/v2/currencies
```

Naming every currency one by one would say something else: a set frozen at the
moment of the ask, which quietly leaves out a currency the guild creates
tomorrow. The collection says all of them, now and later, and it is the same fact
whether a client writes it or writes nothing at all. When the collection and
individual currency URIs occur together, the collection covers all currencies;
every URI is still validated, including guild ownership restrictions.

What is *stored* is the currency id, never the URI. The URI is the wire form, and
storing the id is what keeps a grant working when the site moves.

## Where it is asked for

Both ways of asking — the browser's `GET`・`POST /oauth2/authorize` and the
device's `POST /oauth2/clients/@me/grant-requests` — take the same field:

```json
{
  "resource": ["https://vcrypto.sumidora.com/api/v2/currencies/12"]
}
```

An array of absolute URIs, which is how a JSON request carries the repeated
parameter RFC 8707 describes for form-encoded ones. It is optional, and an ask
that names none is an ask for everything — within the target it already names,
which is the whole of what it could mean — and an ask that names the collection
says the same thing out loud. An application that does not know which currencies
it will work with (a wallet, which learns them from the account it is approved
against) has to be able to ask for all of them; an application that does know says
so, and the screen says it back.

A URI that is not absolute, not this service's, not a currency, or — for a
guild's ask — a currency of another guild, is `invalid_target`: RFC 8707's own
error code for it, and one this service did not have until now.

## What is written down

- `grant_requests.resources` — `bigint[]`, the same shape `scopes` already has, so
  a request carries what was asked for in one row.
- `grant_resources` — `(grant_id, currency_id)`, the same shape `grant_scopes` has,
  so a grant carries what was approved in rows a read can join. A currency's
  deletion retains its approved id in these rows, so deleting the last allowed
  currency cannot turn a restricted grant into an all-currency grant.
  Deleting the grant itself still removes its resource rows.

An empty stored set means "every currency of the target". Omitting `resource`,
supplying an empty array, or including the collection URI all produce that set.
Old grants with no resource rows retain the same meaning.

## What a token may then do

The token's grant carries the currencies, and the acts that move money in one are
checked against it — issuing from a pool, paying, and deciding a claim — whether
the currency is named in the request or is the one on the row the request names.
An act that lands on a currency the grant does not cover is `403
insufficient_scope`: the token is a real one, for something else.

Writes check the currency again inside their transaction. Payments and claim
creation hold key-share locks on the currencies resolved from the units;
issuance checks under the pool's write lock. The locks keep a checked unit or
guild bound to the same currency until commit. A missing currency is refused,
so creation after a lookup cannot silently authorize it. Idempotent payment and
issuance requests perform this check after acquiring the key, including after a
wait for another attempt.

The reads answer differently, and deliberately: a list is *filtered* to the
currencies the grant covers rather than refused, because a list that cannot
mention a currency is no reason to fail a request that did not name one.
`GET /api/v2/users/@me/balances` lists the holdings the token is for, and
`GET /api/v2/users/@me/claims` the claims for them. A read that *does* name a
currency — `GET /api/v2/currencies/:id` — is refused like a write, because there
the request asked for something the token is not worth.

What the resources narrow is a *grant*: a permission somebody else asked for and
somebody else approved. A PAT is not narrowed this way, because it is the
account's own credential rather than a delegation — and neither is the token an
application takes for itself, which is what the issue scope's own token is.

## What the screens show

The common `/grant approve` review displays the target, exact operations and
permitted currencies before its confirmation button grants anything. `/grant user`
and `/grant server` list the corresponding grants and their currency restrictions.
Each device or v3 browser approval creates a separate grant with its own scope/resource pair.
A later approval never changes an earlier grant or its tokens. Long currency lists
are paged in the review and grant Details screens. See
[personal-grants.md](personal-grants.md#independent-approvals) for the model and
legacy authorization-code compatibility.

## Where the pieces go

- `crates/vc-core/migrations/0014_grant_resources.sql` — the column and the table.
- `crates/vc-core/src/grant.rs` — the resource ids beside the scopes in the ask,
  the grant, the decision and the resolution.
- `crates/vc-api/src/routes/grant_requests.rs` and `routes/oauth2.rs` — `resource`
  parsed, and `invalid_target` for what is not one.
- `crates/vc-api/src/resource.rs` — the one function the endpoints share: given a
  unit or a currency id and a resolved token, whether it is inside the grant.
- `crates/vc-api/src/routes/v2/{currencies,transactions,claims,users}.rs` — the
  checks, and the two lists that filter.
- `docs/api.rs`, `docs/issue.md`, `docs/personal-grants.md`, `docs/qa.md` — the
  parameter, the error, and which endpoints rule on it.
