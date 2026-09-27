# Known gaps

## Where the contract comes from

Two clauses of the specification this service was written against shape how
everything is verified:

- *"未知のフィールドは無視しなければなりません(レスポンスへのフィールドの追加や、
  リクエストへの Optional なフィールドの追加は破壊的な変更とみなされません)"* —
  unknown fields are ignored, and adding response fields or optional request
  fields is not a breaking change.
- *"`error_description` フィールドの内容、およびその存在の有無は仕様の範囲外です"* —
  the **content and even the presence** of `error_description` is explicitly out
  of scope. Only `error` / `error_info` and the status code are contractual, so
  the exact wording and ordering of messages such as the metadata validation
  details are not chased. (This is why `Rest.md` can document
  `forbidden`/`invalid_operator` for `GET /claims/:id` while the implementation
  returns `not_related_user`; both are conformant.)

`Rest.md` does not document `GET /api/v2/users/@me` or
`GET /api/v2/users/@me/balances` at all, and the reason is that they are consumed
only by the web frontend. Their JSON is therefore an *internal* shape rather than
a published contract: the goldens are a convenience, not a promise, and both
endpoints may change or disappear alongside the frontend rewrite. Do not spend
fidelity effort on them the way the documented endpoints deserve.

`Authz.md` documents three token kinds — `user`, `app` and `guild` — and says a
`guild` token is what may `give`. The implementation's `verify_claims/2` accepts
only `user` and `app`, so a `guild` token is rejected today.

**No longer true, and kept here because the shape is deliberate.** The rewrite
implements the `guild` kind the specification asks for, but the claim is not what
carries it: the token a guild issues for is the `access_tokens` row the code flow
has always handed out, and its guild-ness is what that row resolves to
(`vc_core::grant::resolve_token`). The kind check `verify_claims/2` would grow is
instead answered by the grant — see `docs/issue.md` for the endpoint, the scope,
and the ask a guild answers. If `guild` tokens are ever issued *as* JWTs,
that check still has to grow with them.

Behaviour that the Elixir service has and this rewrite does not implement yet,
listed here so it cannot be forgotten. Everything here is deliberately deferred;
nothing is dropped by accident.

## The jobs that run on a clock

The Elixir's scheduler is Quantum (`lib/virtualCrypto/scheduler.ex`), and its jobs
are four, from `config/config.exs`:

```elixir
jobs: [
  {"@daily",    &VirtualCrypto.Money.reset_pool_amount/0},
  {"* * * * *", &VirtualCrypto.Auth.purge_user_access_tokens/0},
  {"* * * * *", &VirtualCrypto.Auth.purge_access_tokens/0},
  {"* * * * *", &VirtualCryptoWeb.IdempotencyLayer.Payments.purge_idempotency_keys/0}
]
```

`vc_api::scheduler` runs three of them: the pool refill once a UTC day — the day
claimed in `job_runs` (`vc_core::job::claim_day`) rather than remembered in the
process — with the arithmetic of `vc_core::currency::reset_pool_amount`, the
Elixir's SQL read out of `money/query-service/currency.ex`; and the purge of the
rows whose `expires` has
passed — the signed tokens, the access and refresh tokens, the authorization codes
nobody redeemed, and the idempotency keys (`vc_core::purge`). The codes and the
refresh tokens are an addition to the Elixir's three purge jobs, on the same
`expires` and for the same reason: its list naming three of the five tables that
carry the column looks like a job written in a hurry rather than a decision to keep
the other two.

What is **not** here, and what an earlier version of this file did not say at all —
which is what made a gap read as a feature:

- **The pool refill measures the supply in users' hands**, `SUM(assets.amount)` per
  currency, and not the creator's initial grant, and its allowance is rounded on
  the way into the column rather than truncated (`(supplied + 199) / 200` read as
  `SUM` of `bigint`, which is numeric, is the Elixir's own arithmetic). A currency
  whose users hold nothing is left alone. The *timing* is the database's rather
  than the process's: `job_runs` holds the day the refill last ran, claimed in one
  statement, so a restart — or a second scheduler behind the same database — cannot
  buy another day's allowance. It is a table of this service's own, which the
  Elixir's Ecto neither knows nor needs; what the shared schema cannot carry is a
  marker *column*, which is why the day is not on `currencies` and why the claim is
  asked every tick rather than remembered in two places. The behaviour is locked by
  tests (`crates/vc-api/tests/pool_refill.rs`): the allowance, the ceiling, a
  currency nobody holds, and a day claimed once.
- **Re-verifying applications whose webhook has gone quiet.** The Elixir checks a
  webhook at registration and patched edits; whether anything re-runs that
  periodically, and what a failed re-check would do, is not in its scheduler's job
  list and not in its source as read so far.

## Discord lookups are cached in the process

`Discord.Api.Cached` wraps the raw API and remembers `get_user` and `get_guild`
for fifteen minutes, a 404 included — the same shape as the Elixir `Cachex`
tables, and what keeps a page of claims from asking about the same missing user
once per row.

Like Elixir's, a miss is fetched once: callers that want the same id while it is
being looked up wait on that id's lock and then read what the first one stored,
rather than each making their own call.

Two differences from Elixir's, both about the bound rather than the behaviour:

- The table is bounded (ten thousand entries) and gives up its oldest tenth
  rather than clearing, so a long-lived server cannot grow without limit and a
  full table does not send every caller back to Discord at once.
- It lives in this process, so every Fly machine keeps its own. That changes
  nothing a caller can see.

## Rate limiting is per caller, and only where the caller is known

The heading here used to say "only on the interactions endpoint", which stopped
being true when `routes/limited.rs` arrived: what is left of it is the reason the
limit is where it is, which is that a caller is only known once something has
established who they are.

Requests are counted per identity rather than per address, because an address
says nothing once authentication has decided who is calling: `RATE_LIMIT_PER_MINUTE`
(default 120, zero disables it) applies to the Discord user behind an
interaction, established by the signature check that runs before it.

The v2 REST API takes the same extractor an endpoint does when it needs it, in
`routes/limited.rs`: `Limited(AuthUser)` counts per account `v2:{subject}`, and
the guild endpoints count per application `guild:{application_id}` in the guild
token's own extractor. Every v2 handler takes it, so the v2 side is covered;
what is *not* is the OAuth2 surface — registration, the edit, and
`/oauth2/clients/@me` take `AuthUser` — and that is deliberate rather than
unfinished: registration's real cost is the handshake, which has a limiter of its
own, per requester and much tighter.

## Behaviour warnings are observed per process, and only where a subject is known

`crates/vc-api/src/security.rs` counts the refusals this service already makes —
429s, `invalid_token`s, failed `same_own` checks, refused Discord callbacks,
bad interaction signatures, duplicate receipts — plus the whole service's error
rate and request volume, and warns the operator once per subject per window
(under the tracing target `vc_security`, and to `VCRYPTO_SECURITY_WEBHOOK_URL`
when one is set). What it does *not* do, and why:

- **No address-based keys.** A subject is the account, application, or Discord
  user when something has established one, and a fixed label
  (`discord-callback`, `interaction-signature`, …) when nothing has. The
  anonymous attempts a label groups — CSRF probes, forged interaction
  signatures — cannot be told apart by *who* sent them: this server does not
  take `ConnectInfo`, and behind Fly the peer address is a proxy while
  `X-Forwarded-For` is caller-controlled unless a trusted-proxy layer parses
  it. Per-address counting is therefore still a gap, same as it is for the
  rate limiters above.
- **Counts are in memory, per process.** A restart loses counts that had not
  reached their window's end, and every Fly machine counts its own — the same
  property the rate limiters have. Emitted webhook reports are saved to
  `security_webhook_queue`; both pending reports and retry times survive restarts.
  If the database cannot accept an enqueue, the incident and the enqueue failure
  remain in the log.
- **Delivery is at least once.** A database lock prevents concurrent senders,
  but a crash after remote acceptance and before the queue row's deletion commits
  can cause a repeat. Success deletes the row; permanent failures are kept for
  seven days by the normal purge, while pending retries are preserved.
- **It warns; it never refuses.** The loose limit still decides what a caller
  is answered, and the tighter `VCRYPTO_WATCH_LIMIT` window beside it only
  counts. Nothing here changes a response.
- **A duplicate receipt needs ten before it speaks.** Discord retries on its
  own and a pending receipt answers `409`, so `ReplayDuplicate` has a threshold
  of ten per window rather than one; every other signal warns on the first.

## Not yet verified against captures

- The pagination `link` header on `GET /api/v2/users/@me/claims` follows the
  example in `Rest.md`: `<scheme>://<Host verbatim, port included><path>?…`, with
  the query rebuilt as `type`, `order`, `next`, `limit`, `related_*`, then
  `statuses[]`. The tests assert that a full page carries the header and a partial
  page does not; asserting the exact URL is still to do.
- `related_discord_user_id` resolves an existing user, while Elixir's resolver
  may create one. A freshly created user has no claims, so the filter result is
  the same either way, but the side effect differs.
- An unparsable cursor value and an unknown `order` used to answer 500, matching
  the crashes Elixir hits in `parse_order/1` and in Ecto's `bigint` cast. **They
  are 400s now**, which is a deliberate difference and has moved to "Deliberate
  differences" below.

## Documented deviations from captured behaviour

These are intentional and non-breaking; each is also recorded next to the
relevant test.

- Response headers that are incidental to Phoenix (the `charset` parameter,
  `cache-control`, `x-request-id`) are not replicated.
- Auth-error responses gain a JSON `content-type`, which Elixir omitted only
  because it wrote them with `Plug.Conn.send_resp`.
- `GET /api/v2/users/@me/claims/:id` answers `404` for a non-numeric id, where
  Ecto's `bigint` cast would raise a 500. `PATCH` already answers an unparsable
  id with 404, so the two endpoints stay consistent.
- The `APPLICATION_COMMAND_PERMISSIONS_V2` branch is gone. Elixir required the
  administrator bit only from guilds carrying that feature, and let every other
  guild run `create` and `give`; Discord finished that migration, so no such
  guild is left and the branch could no longer be false. Removing it also
  removes the guild lookup those two commands made solely to read the feature
  list. `create_test.exs`'s "not admin without v2 flag" case passes the default
  permissions — every bit set — so it still passes; its name is the only thing
  that referred to the flag.
- The claim list's `:last` page is resolved by counting the matching claims, so
  it really is the last page. Elixir's `Raw.Get` clause for `%{page: :last}`
  fetches one page's worth of rows in ascending order and reverses them, which
  only lands on the last page when there is a single one — with more, it
  re-renders the first. No Elixir test covers it, and the button that asks for
  it is disabled while there is one page, so this diverges from a path the suite
  does not pin.

## Deliberately out of scope

Two items in the original plan were dropped by decision, not forgotten:

- **`GET /api/v2/users/@me/balances` was one of them, and is implemented now** —
  the wait was for the frontend, and the endpoint arrived with it
  (`routes/v2/users.rs`, and four goldens under `tests/golden/`). What an
  application may see about the people it deals with is the contract it holds with
  them and nothing else: the separate balance read this paragraph used to point at
  is gone (`docs/contracts.md` says why).
- The differential harness — replaying a recorded request corpus against both the
  Elixir and the Rust service — is not built. The ported contract tests and the
  captured goldens are the evidence of compatibility instead.

CI is no longer one of these. `.github/workflows/ci.yml` brings up a Postgres 17
— the major version the baseline was generated from — and runs `just check`,
which is the same four gates a laptop runs.

This paragraph used to end "so no `.sqlx` offline cache is committed and none can
go stale", which was true of the CI that compiled against a live database and is
not true of this one: `.sqlx/` is committed, `just sqlx-check` is one of the gates
(`cargo sqlx prepare --workspace --check -- --all-targets`), and a query that is
added without regenerating it fails the check rather than the deployment.

## OAuth2 and applications

**Built, and this section claimed otherwise for longer than it should have.** The
routes are all in `routes/mod.rs`: `/oauth2/authorize` (the consent screen),
`/oauth2/token` (the code exchange, the refresh, `client_credentials` and the
device poll), `/oauth2/token/revoke`, `/oauth2/clients` (list and register),
`/oauth2/clients/@me` (read and edit) and `/oauth2/clients/@me/grant-requests`.
`docs/oauth2.md` is the contract read out of the Elixir's controllers and the port
follows it; what the Elixir has **no tests for** is covered by this tree's own
suite instead — `tests/oauth2_*.rs` and `tests/interactions_*.rs` — which is an
addition rather than a port, and `docs/test-port.md` is where the difference is
recorded.

The claim notifications were waiting on this, and are not any more: the handshake
registration performs is the transport the notifier sends through, and both now
go through `notification::check_webhook`.

## Discord interactions

`POST /api/integrations/discord/interactions` verifies the Ed25519 signature over
`timestamp <> body` and answers PING (type 1) with a PONG. Types 2 (application
commands), 3 (message components) and 5 (modals) are dispatched by name or by
`component_type`, with the state each one acts on packed into its `custom_id`.
Type 4 (autocomplete) is dispatched on the focused option's name and, for a
claim id, the command path — the same `id` offers received-and-pending under
`approve` and `deny`, claimed-and-pending under `cancel`, and every status under
`show`. Elixir has no test for any of it, so `tests/interactions_autocomplete.rs`
holds additions rather than ports.

One thing it cannot name yet: a claim whose party is an application rather than a
discord user reads as `deleted`, where Elixir shows that application's client
name. That needs the OAuth2/application side.

One deviation: a command name that no `Command.handle/4` clause matches answers
400 `Type Not Found`, where Elixir has no clause either and so raises, producing
a 500. Discord only sends registered command names, so this is unreachable in
practice; it is recorded because it is not a faithful reproduction.

## Deliberate differences

### A credential the Elixir had no notion of: the personal access token

`/pat create|list|revoke` make a token an account can hand to something that is not a browser, and
it is an addition in the plain sense: the Elixir has no personal access token, no API key, and no
column that could hold one. There is no golden for it, `docs/pat.md` is the design, and
`crates/vc-api/tests/pat.rs` is this tree's own suite.

What it is, in one sentence: a `kind: user` token carrying the browser session's scopes, with a name
and no lifetime, revoked by deleting the `user_access_tokens` row every JWT here already resolves
against. The API therefore sees no fourth kind of caller, and the only thing the reference had to
learn was that a user token can also come from `/pat`.

That no lifetime is the one claim this service writes that Guardian never did, and it is confined:
a personal access token carries no `exp`, and the verifier checks an `exp` that is there rather than
demanding one. Everything else — a session's hour, an application's hour — is unchanged, and the
purge job skips a row with no `expires` because SQL compares nothing to `NULL`.

The scope list is the browser's, which is why a PAT cannot create a contract: the application half
of the contract family takes an `app` token, and no scope changes a kind. The party half —
`approve`, `refuse`, `withdraw`, reading, and its own list — takes a user token and no scope at all,
so a PAT does all of it, and `crates/vc-api/tests/pat.rs` pins that it does.

### A list a reader can filter: the mute

`/mute currency`, `/mute user`, `/mute list` and `/unmute` let an account leave a currency or
somebody out of its own claim and contract lists, and it is an addition in the plain sense: the
Elixir has no mute, no column that could hold one, and nothing that filters a list by its reader.
`crates/vc-core/migrations/0019_mutes.sql` is the table, `vc_core::mute` is the domain, and
`crates/vc-api/tests/interactions_mute.rs` is this tree's own suite.

What it is, in one sentence: a row of (who is looking, what they are not looking at), which the
claim and contract lists ask before they answer a page — for that reader's lists only, in Discord
and in the API both, because they are the same lists.

Three things about it are decisions rather than details:

- **Nothing is forbidden.** A muted currency can still be issued, claimed, paid and contracted
  into, and a muted person is refused nothing: what a mute changes is what the *reader* is
  shown. That is why the command's own prose says so, and why no endpoint answers a 403 for it.
- **A page and its count agree.** The filter is in the statements rather than in the callers, so
  `:last` is the last page of what the reader can see and a full page is a page with more behind
  it. A filter applied after the read leaves an arrow leading to an empty page.
- **A single row is not filtered.** A claim by id, a contract by id and a contract's payments
  answer what they are asked for; only the lists leave rows out. A mute is a preference about a
  list, not a permission over a row.

There is no end date. The row is deleted by the person who wrote it — from `/mute list`, or by
`/unmute` — and no job would take it away; that is the whole of the lifetime, and it is why the
table has no `expires` column.

### A ledger a person can read: `/history`

`/history pay` and `/history issue` show back the two ledgers this service has been writing since
the Elixir: what one account paid and was paid, and what one guild's pool issued. The person's is
both ledgers at once, because an issuance to somebody is money arriving in their wallet like any
other, so the payments screen shows the payments and what the pool issued to the reader, merged
newest first; the guild's issuance ledger is one table and is not merged with anything.

Contract movements identify the application by its bound Discord Bot, as the approval
screen does. An unbound application shows its sanitized name and client ID; editing
the name cannot insert mentions or extra movement lines into old history.

The Elixir writes both and reads neither. Its command list — `help invite give pay info create
delete bal claim` — names no history; `lib/virtualCrypto_web/controllers/api/v2/user_transaction_controller.ex`
and its v1 twin have a `post` and nothing else; and no web page reads either table. There is
therefore nothing to port: `vc_core::history` is the read, `crates/vc-api/src/command/history.rs`
is the screens, and `crates/vc-api/tests/interactions_history.rs` is this tree's own suite. The
tables are the Elixir's, unchanged — no column was added for this.

The ledger gained two kinds of row, and they too needed no column. Locking money into a contract
(`contract::approve`) and returning what was left of it (`contract::withdraw`, `contract::settle`,
a refusal, an expiry) each move a balance, and neither wrote anything — so the largest movements of
a person's money were the invisible ones, while the charge that spends the locked money was
already recorded. Both are now written into `currency_payment_histories`, and **the NULL side is
what tells the three contract movements apart**: a charge sets both `sender_id` and `receiver_id`, a
lock names only the party who approved and leaves the receiver NULL because the escrow that took
the money is nobody's account, and a return names only the party it went home to and leaves the
sender NULL. A person's ledger is the wallet's own movements, so it shows a lock and a return.
A charge appears only in the receiving wallet, with `event: "charge"`: it increases that
wallet's balance even when the receiver is not a contract party. The paying party already
recorded the lock, so the charge is not another debit in their wallet. The application
reconciles the escrow from the contract's own statement
(`GET /api/v2/contracts/{id}/payments`), which shows all three and names which with `event`. A
movement of nothing writes no row, and each is written where the money moves, in the same
transaction that moves it.

**A third thing, and a repair rather than an addition.** An application may name one of the
contract's own parties as the receiver of a contract payment, and that moves money out of the
escrow into that party's wallet — which is the party's balance moving, whatever it is called. The
row was written charge-shaped, with both sides set, so the wallet history, which believes the NULL
side, treated it as a charge and left the money arriving out of the list entirely. It is written
the way a return is written now — `sender_id` NULL, the party's own account on the receiver side —
because that is what happened. The application-receiver case is untouched, and so is the refusal
that stops one party's remainder being paid to another person where the receiver is fixed. One
escrow paying one wallet is **one row for the whole movement** rather than one per remainder drawn
on: the sender side is the escrow, which is nobody's account, so which slice came from where is not
a thing such a row could say.

Two things about it are decisions rather than details:

- **What may be seen follows what may be done.** The payments screen is the caller's own rows and
  needs no permission; the issuance screen asks for the administrator bit `/issue` asks for, read
  the same way in the same handler, because the pool is the guild's and who it paid is the business
  of whoever may spend it.
- **Read by its owner, in Discord and in the API.** `/history pay` and
  `GET /api/v2/users/@me/transactions` are one list — the caller's own account, and nothing beyond
  a token that may pay, because a caller that may pay may read what it paid. The guild's side is
  the same pair: `/history issue` and `GET /api/v2/currencies/{id}/issuances`, gated the way
  issuing is. Neither is in `Rest.md` and the Elixir has no route for either — an earlier version
  of this file said the person's ledger was Discord-only on the grounds that an application has no
  business reading it, and what changed that is the payments endpoint being the path the payment
  writer already lived on.

**The person's list is the one list here whose cursor is not an id.** Its two tables count their
ids separately, so "newest first" has to be a place both have: `"time"` descending, the ledger
(`payment` = 0, `issuance` = 1) and then the id breaking the tie, because `"time"` is whole seconds
and ties are ordinary. The cursor is that place, spelled `2026-01-01T00:00:00Z:1:5`, and it is a
string where every other list's is a number: `docs/api.rs` documents the shape under `next` and
`on_next` of that endpoint, and `vc_core::history::Place` is where it is spelled and read. A
counterparty filter narrows an issuance out — the pool is not the person the caller asked about —
and the count on the Discord screen is the merged list's, not one table's.

### The consent screen's scope is `vc.issue`, where the Elixir's was `openid`

`is_valid_scopes?/1` accepted `openid` and nothing else, and this port did the same until now.
It is gone: this service is not an OpenID provider — it issues no `id_token`, has no `userinfo`
and no `jwks`, and no route, extractor or token check reads a scope of that name — so an
application was handed a permission that granted nothing, and the page that listed it said
「利用者の情報を読む」, which was not true of any endpoint here.

What a consent screen may ask for is `vc.issue`, the scope that gives the code flow's guild token
something to do, and nothing else about the flow moved: the exchange, the grant and the token are
the same. A request asking for `openid` is answered `invalid_scope`, which is what the Elixir
answered for every other name.

The schema keeps the value — `virtual_crypto_scope_type` is the baseline's and `'openid'` is one
of its members, which `just baseline-check` holds the migrations to — and a grant written before
this may still carry the name. Nothing reads it.

### A party's *wallet* is not readable through a contract

`GET /api/v2/contracts/{id}/balances` answered each approved party's `assets.amount` — what they
hold in the contract's currency, not what the contract holds. It is gone, and `docs/contracts.md`
says why: the number an application decides with is the contract's own `remaining`, which every
contract read already carries, so the separate read decided nothing and showed a user's wallet to
an application they had only lent one amount to.

The rest of the contract family is unmoved: `GET /api/v2/contracts/{id}` answers the contract's
`remaining` and each party's `amount`, `remaining` and `status`.

### A client's typo is a 400 here, where the Elixir answered 500

Three things a caller can get wrong were answered with a 500 — a status that says
the *service* failed, about something the caller can fix without help. Each is a
400 that names it now:

- **`limit=-1`** reached Ecto and came back as an error of the database's. It is
  `invalid_limit`, which is what the non-numeric limit beside it was already
  answered with.
- **`next=abc`** (or `on_next=abc`) went into Ecto's `bigint` cast and raised. It
  is `invalid_cursor` — the same complaint two cursors at once already got, since
  both say the caller named a place to resume from that cannot be read.
- **`order=nonsense`** found no clause in `parse_order/1` and raised. It is
  `invalid_order`.
- **`limit=1000000`** would have been passed to Postgres as a page size and
  answered with a million rows. Past two hundred it is refused (`invalid_limit`): a
  page size the service must honour is a request to read a table, and a caller given
  a smaller page than it asked for may take that page for everything it asked for.

**This reverses what this file used to say**, which was that such client errors
"are reproduced rather than invented differently". What decided it is whose error
a status reports: a wrong cursor says nothing about this service, and the caller
who can fix it is looking at the status. All three are pinned by tests on the
claim list and on the contract lists, which share the reader — so they are
behaviour rather than an accident of a column.

### An absent `limit` is a page, where the Elixir answered every matching claim

`GET /api/v2/users/@me/claims` answered its whole result set when the caller gave
no `limit`, which is what `Rest.md` says it does. Every other list here — the two
contract lists and a contract's statement — answers fifty rows and a `link` header
to the next page, and a list that reads a table in proportion to a caller's data
is the one thing a list endpoint must not do. The claim list takes the same fifty
now (`routes/pagination.rs`'s `PER_PAGE`, one number for all four lists, with the
ceiling of two hundred beside it).

Records that were written against the old behaviour and read the endpoint in a loop
without a `limit` will see fifty rows and no error; the `link` header is how the
rest is asked for, and a caller that asks for a page size still gets it.

### A malformed `amount` is named after the field, not after another clause

The single-payment clause of `POST /api/v2/users/@me/transactions` answered a
non-numeric `amount` with `invalid_format_of_convert_amount`: the name of the
*list* clause's amount, and a name that points at a field neither clause has — the
list clause reads `amount`, `unit` and `receiver_discord_id`. The charge and issue
endpoints answer the same mistake with `invalid_format_of_amount`, and this one
does now too.

The status was 400 before and is 400 after; only the description string moved. The
Elixir's own wording for this clause cannot be checked from here (`Rest.md` is not
in this repository), so this is recorded as a deliberate difference rather than a
correction.

### The claim list's rows act, and nothing is selected

The Elixir's list answered a multi-select menu and one action row that appeared
once something was chosen: a claim could be ticked, several could be approved or
refused at once, and a quotation line said what the selection would spend. The
port kept that and has now dropped it. Every **pending** claim carries its own
`✅`／`❌`／`🗑️` row, under the same rules the claim's own screen uses — an
approval is the payer's and only with the money, a refusal is the payer's, and
taking the claim back is the claimant's — and a claim that is already decided
carries none, because a screen that offers what it will refuse is a screen that
lies.

What is gone with it: the menu, the selection screen with its quotation, and the
`☑`／`◻️` marks a ticked row used to carry. The action row's ids (7 and 11,
`Act::Back` and `ActionSingle::Back`) are refused rather than reused, because a
message already in a DM carries them. Bulk approval is still one request
(`vc_core::claim::update_claims/2` takes a list), but the Discord surface no
longer offers a way to build one: a page of five is four presses fewer than it
was, and the person pressing them sees which claim each press is about.

### The claim is in the write's transaction

The Elixir's idempotency plug claims the key and *then* calls the controller, in a
transaction of its own — so a process that died between claiming and answering left
a row with nothing under it, and a retry was answered `409 processing` about a
request that was over until the purge took it a week later. This service claims
inside the write's transaction instead: the row, the write and the answer are one
commit, which makes that state unreachable rather than rare. A transaction that
rolls back — the database failing, a serialization failure — takes its claim with
it, so the caller may use the same key again.

Three things follow from it, and all three are pinned by tests:

- A request that never became a write does not spend the key: the body is read
  before the key is claimed, so a value that does not parse is refused without
  touching it.
- Two requests with one key at the same time serialize on the key's unique index —
  the second waits inside its insert and then reads the first one's answer, or
  takes the key over if the first rolled back. **The wait is capped at a second**:
  a request still waiting is answered `409 processing`, so a slow write costs its
  retries a second rather than holding them behind it. **The transaction names
  `READ COMMITTED`** rather than inheriting it, because the waiting is a property
  of that level: at `REPEATABLE READ` and above the same insert is refused
  outright (`40001`) — the row it conflicts with is newer than the transaction's
  snapshot — and the answer a retry got would depend on a setting nobody here
  chose. That refusal is therefore not handled anywhere: it cannot happen, and
  `tests/contract_idempotency.rs::a_stricter_session_default_does_not_change_the_answer`
  is what says so (it answers 500 without the named level).
- `409 processing` is not "the request is over, come back tomorrow": it is what a
  request is told when it has waited a second for a key another one is using
  (`CLAIM_WAIT`), or when it meets a row that answers nothing — one claimed by a
  version of this service that claimed outside the transaction. Nothing new writes
  one of those, and the row is not the caller's to take over: nothing can say
  whether the request that made it wrote.

### A key is not compared against the request that carries it

The specification for this header suggests answering a key that is reused for a
*different* request with `422`. This service does not: a key that already answered
is answered from the key, and the body arriving under it is never compared with the
body that spent it.

Replay still requires the same operation (wallet payment, guild issuance, or
contract payment) and, for a delegated credential, the same grant. Reusing an
account's key across those boundaries returns `409 idempotency_context_mismatch`
with `Idempotency-Status: Duplicate`, without disclosing the cached body or
executing the new request. Access-token refresh preserves the grant and therefore
preserves retries. This authorization check does not compare request bodies.

Migration `0023` adds that replay context while keeping the existing account/key
uniqueness rule. Old completed rows have no recorded context: only `201 {}` may
be replayed to the account's own wallet-payment credential. Other completed legacy
rows return the same context conflict and retain their key until normal expiry;
discarding them could execute a completed payment twice. Legacy pending rows keep
their existing `409 processing` response. A context conflict is not an instruction
to retry with a fresh key: clients must check the original operation's outcome.

The reason is that the specification does not say what makes two requests the same,
so the comparison would be invented here — and every version of it is wrong in both
directions. Compare too much and an honest retry is refused: a bulk list sent in
another order (the list is atomic, so the order means nothing, but it is in the
bytes), a field this API ignores left out, an amount written in another way. Compare
too little and the case the check exists for slips through anyway. The one thing a
server cannot know is the caller's intent, and that is the whole of what the
question asks, so the rule is left where the specification left it: the key is the
caller's own word for one request, and a caller that reuses it for another request
is shown the first request's answer when the replay authority matches.

The cost, stated rather than hidden: a client that reuses a key for a genuinely
different charge is answered with the *first* charge's answer, and its second charge
does not happen. Nothing on this side can tell that apart from a retry — the caller
is the only one who knows — so the place to notice is its own statement,
`GET /api/v2/contracts/{id}/payments`, or the answer it was handed, which carries
the first request's numbers rather than the second's. `docs/contracts.md` says the
same thing where clients read it.

### The currency command is `/issue` here, where the Elixir's was `/give`

Renamed, not reimplemented. The Elixir registered `give` in
`priv/register-commands.exs` and dispatched it in `Command.handle/4`, but the call
behind it was `Query.Issue.issue/3` all along — so this service registers and
dispatches `issue`, and the domain function is `vc_core::issue::issue`. The
options, the administrator bit, the guild requirement and the answers are
unchanged; `tests/interactions_issue.rs` is the port, and `docs/test-port.md`
records that Elixir had no test for it either way.

The rename stops at the command. The table the operation writes keeps the Elixir's
name, `currency_given_histories`, because the baseline is the v2 Ecto schema and
both services share it.

Registering the new name is what removes the old one — Discord's `PUT` to
`/applications/{client_id}/commands` is a bulk overwrite, so `just register-commands`
puts `/issue` up and takes `/give` down in one call. Until it is run Discord still
offers `/give`, and an interaction for it is answered `400 Type Not Found`, since no
`Command.handle/4` clause matches that name any more.

### The login redirect is 303 where Phoenix sent 302

`Redirect::to` in axum is `303 See Other`; `Phoenix.Controller.redirect/2` sends
`302 Found`. For a browser navigating a `GET` the two behave identically, which
is why this was accepted rather than hand-building a 302 — and for the consent
screen's `POST`, which is the case that follows, 303 is the correct answer rather
than merely an acceptable one.

### `POST /token` without a session answers 401

The Elixir controller's `token/2` returned `nil` when the session had no user,
which in Phoenix is not a response at all — the request fails rather than being
answered. Nothing depended on that, and a status is something a caller can act
on, so this answers `401` with the same `invalid_token` body the rest of the API
uses.

### A member's unknown role id is skipped, where the Elixir crashed

`validate_executor` maps the member's role ids through the guild's roles and calls
`String.to_integer/1` on each result, so a role id the guild does not list raises.
That is reachable, not theoretical: the member and the guild's roles come from two
separate Discord calls, and a role deleted between them lands exactly here — which
makes it a bug rather than a branch, and one that a retry would not fix.

The Rust side skips the id, so the same race costs a permission check that may be
a moment stale instead of the request.

### The consent screen's two repairs

`validate_executor` asked Discord about the session's user id, which is a
VirtualCrypto id and not a Discord one, so the lookup could not succeed and the
Approve button was unreachable. It now reads the account's `discord_id` and asks
with that.

A missing session raised, which is a 500 where the answer is a trip to
`/login?continue=…`. That repair needed the login page to exist, which is why it
waited until it did.

### `POST /oauth2/token` answers 400 where the Elixir answered 200

`TokenController` renders the code exchange's error bodies without setting a
status, so they arrive as `200` with an error in the body. RFC 6749 has
`invalid_grant` at `400`, and a client library that checks the status before the
body — which is most of them, and which is what this endpoint exists for — reads
the Elixir's answer as a success. The refresh and `client_credentials` paths in
the same controller do set `400`, so the code exchange was the odd one out.

Code and refresh-token exchanges read the clock after locking the credential,
including the grant lock taken before a refresh. A credential that expires while
waiting is refused without consuming the code or rotating the token. This is a
boundary-condition change for delayed exchanges; the existing second-precision
comparisons and token lifetimes are unchanged.

## The notification transport, as far as it is specified

`docs/api/Webhook.md` and `notification/webhook/cloudflare-workers.ex` together
say what the delivery is, and it is smaller than the earlier note in this file
assumed.

**The protocol** is Discord's, deliberately. Events are a claim being approved or
denied; a body is `{ "type": 1 | 2, "data": ... }` with 1 as PING and 2 as a
claim update, and **unknown types must be ignored**. The signature is ed25519 over
the timestamp and the body concatenated as bytes, in `X-Signature-Ed25519` as hex
and `X-Signature-Timestamp` as digits — the same thing this service already
verifies on its own interaction endpoint, which is why `ed25519_dalek` is already
in the tree. An application that cannot verify must answer `401`, and
**VirtualCrypto checks that it can at registration and periodically afterwards**.

**The delivery goes through a Cloudflare Worker, and that worker is protected by
mTLS** — so a client certificate belongs on this side, and an earlier version of
this paragraph was wrong to say otherwise. The Elixir reads a `webhook_proxy` from
configuration and posts there; the worker presents a certificate of its own and
requires one from this side. That is the whole of the protection, so a client
without the certificate is not a client the worker will talk to.

`reqwest` takes one as an identity — `Identity::from_pem` over the certificate and
its key together — and that needs the TLS feature to be chosen deliberately rather
than left at the default.

The next thing to read is that worker's own request shape; its source is
`webhook-emitter-cf-workers` in the same organisation.

**The keys are per application**, and are the reason `applications` has both
`public_key` and `private_key` as `NOT NULL`: the service signs what it sends with
the private half and the application verifies with the public one. That is also
why the fixture in `tests/oauth2_preauthorize.rs` has to write both, which the
runtime taught us rather than the schema.

The delivery is implemented: `WebhookNotifier` signs with the application's
private key and posts through the worker where one is configured, and straight at
the application's own `webhook_url` where there is none — the same choice the
handshake makes.

**And so is the re-verification, which this paragraph used to say was missing.**
`handshake` ("verify") runs when a webhook is registered or edited, and
`scheduler::reverify_webhooks` runs it afterwards: one tick's work is a batch of
the applications whose webhook is due, and the outcome goes into
`applications.webhook_verified_at` and `webhook_failed_at` (`0010`), which
`GET /oauth2/clients/@me` answers back to the application. What makes a webhook
due is `applications.next_reverify_at` (`0016`): each check sets it a week out,
and registering or replacing a webhook sets it to now. Results are saved only
while the URL, signing key and webhook revision still match the checked snapshot
(`0027`). Changing the URL or recording a check advances the revision, so a late
result cannot overwrite a replacement's schedule, including when the URL is
changed back within the same second. Unrelated metadata edits preserve the revision.
A failure is
recorded and logged and that is all — the webhook is not taken away and deliveries
keep going, because what a delivery carries is a decision about somebody's money
and an outage is not a reason to stop telling an application about it. The
sentence that stood here — "nothing runs it afterwards, because there is no
scheduler in this service at all" — was true when it was written: the clock that
settles contracts is newer than it, and the re-check is the fourth job on it.

### What a delivery is, and what the handshake checks

Read out of `CloudflareWorkers`. The proxy is generic: **the target is in the
request**, and the proxy answers with what the target said.

A delivery is a `POST` to the configured proxy whose body is the event's JSON and
whose headers are:

| Header | |
| --- | --- |
| `X-Signature-Ed25519` | ed25519 over `timestamp <> body`, **hex and lowercase** |
| `X-Signature-Timestamp` | the time in whole seconds |
| `X-Forward` | **the application's `webhook_url`** — where the proxy should send it |

and the signature is made with the **application's** `private_key`, not with
anything of this service's. The response carries `X-Status`, the status the
target answered with, plus the target's body — so the caller learns what the
application said without the proxy having to paraphrase it.

The handshake is **two requests, not one**, and that is the part worth not
simplifying:

1. a PING — `{"type": 1}` — signed with the application's real key must come back
   `200` and a body of `{"type": 1}`, which says the application answers webhooks
   at all;
2. a PING signed with a **keypair generated for the occasion** must come back
   `401`, which says the application actually verifies what it is sent.

An application that cannot tell a real signature from rubbish fails the second and
is refused. Two results must both be `:ok`; a wrong answer is
`verification_failed`, a proxy that could not be reached is
`internal_server_error`. This service draws the same line in its own words —
`webhook_verification_failed` for the application's failure, `server_error` for its
own — and a silence with no proxy in the way is the application's, because there is
then nothing between this service and the webhook for it to belong to.

The two requests are **shuffled** so that the order is not something to guess at,
and `verify` draws which PING goes first for that reason: an application that
answers `200` to the first request and `401` to the second, checking nothing,
passes only the handshakes whose real PING happened to be the one that went first.

The handshake is also rate-limited **per requester** — one per three seconds,
twenty per hour, fifty per day — answering `retry_after_3_seconds`,
`retry_after_1_hour` or `retry_after_1_day`. `rate_limit::VerificationLimiter` is
that limiter, charged to the account that would be asking twice, by registration
and by the edit.

The client certificate lives in this module's own configuration under `:ssl`,
which is the mTLS the worker demands. And a notification is **fire-and-forget**:
`Task.start`, so a slow application does not slow the claim that caused it, with
an account that has no application being a silent `:nop`.

## The v2 rate limit, and why it is an extractor

**Planned here, and built since.** What follows was written as the argument for a
shape that has arrived — `routes/limited.rs`, taken by every v2 handler — and it
is kept because the reasoning is the part worth having; the paragraphs below that
describe the work as outstanding are marked where they are.

The limiter already exists and the interaction endpoint already uses it:
`RateLimiter::allow(&self, key: &str) -> bool`, keyed per caller — the interaction
side asks for `format!("discord:{user}")`. What v2 had not got was the calling of
it, and where to call it decides how much code that is.

A layer on the v2 router is the wrong place. The limit is per **account**, and an
account is only known once the token has been verified — which happens inside the
handlers, through the `AuthUser` extractor. A layer would therefore have to verify
tokens a second time, or limit on the address, and the Elixir counts the caller.

The right place is an extractor of the same kind: one that verifies the token and
then asks the limiter, so a handler takes `Limited` instead of `AuthUser` and
cannot forget to be limited. It has to be `FromRequestParts<AppState>` rather than
generic over `AuthState`, because the limiter is the state's and `AuthState` — the
pool and the signing secret — does not carry it.

That was a parameter change in every v2 handler: mechanical, about twenty of them,
and the compiler found them all. The refusal is the one the interaction endpoint
answers with, and `RATE_LIMIT_PER_MINUTE=0` still turns it off.


## Payment requests require JSON

`POST /api/v2/users/@me/transactions` intentionally accepts only JSON request
bodies. Supporting form-encoded payments is not intended for this port.
The Elixir endpoint's `Plug.Parsers` also accepted
`application/x-www-form-urlencoded`; existing clients using that encoding must
send `Content-Type: application/json` and a JSON body instead. Form-encoded
requests receive HTTP 415. This is an accepted compatibility gap, not an
outstanding bug. The token and revocation endpoints' form support is separate
and remains supported.

## Related-user IDs reject signs

For `GET /api/v2/users/@me/claims`, `related_vc_user_id` and
`related_discord_user_id` must be positive ASCII decimal IDs within the supported
integer range. Leading `+` or `-`, zero, and whitespace are rejected with HTTP 400
(`invalid_request`, `invalid_related_user`). Unlike Elixir's general integer
parser, this intentionally treats the parameters as IDs rather than signed
numbers. A leading `+` is rejected, not normalized in pagination links.

## Discord profile failures do not fail claim responses

Claim creation and approval commit before the API builds its response. Failing
that response because Discord is unavailable can make a successful payment look
unsuccessful, or cause a retried creation to create another claim. Claim reads,
creation, and updates therefore return a minimal `discord: {"id": "..."}` when
profile lookup fails or returns 404. The ID comes from the stored account; an
account with no Discord ID still returns `discord: null`. Successful lookups
continue to return the filtered profile. This deliberately avoids the old
serializer's failure on unavailable profiles.

## User profiles without a browser Discord authorization

Personal access tokens can be issued entirely in Discord. Application registration and
`GET /api/v2/users/@me` therefore use the bot's user lookup when the account has no stored
Discord OAuth authorization or that authorization cannot fetch a profile. A working OAuth
profile lookup and its refresh behavior remain unchanged. Registration still requires a
successful profile lookup and rejects bot accounts; the owner is the authenticated account's
stored Discord id. This extends the old browser-dependent flow to support PAT callers.

## Redirect URI length limits

Registration and updates now reject redirect URIs longer than 255 Unicode characters and
URI lists longer than 2000 characters including the newline between entries. Both return
400 `invalid_redirect_uri` before writing. These limits also apply to Discord edits and
prevent the modal from silently truncating an existing list. The individual limit matches
the database column; the aggregate limit is new.

## Discord IDs on money-writing endpoints

Single and bulk payments, issuing, claim creation, and contract creation/payments
require positive ASCII decimal Discord IDs within the supported bigint range.
Zero, negative IDs, explicit signs, whitespace, and overflow are rejected with HTTP
400 before any accounts, balances, claims, contracts, or idempotency keys change.
This intentionally tightens v2's signed-integer parsing; amount parsing is unchanged.
The check validates the ID representation, not the existence of a Discord account.

## Persistent browser login is retired

`GET /login`, `GET /logout`, and `POST /token` now return 410. Browser authorization
instead verifies Discord identity for each request and uses a ten-minute,
single-use, browser-bound authorization flow. It stores neither a reusable login
session nor Discord access/refresh credentials. This supersedes the historical
session and `/token` behavior described earlier in this document. PATs and the
application/device token flows remain available.
