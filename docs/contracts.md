# Contracts: letting an application operate a user's currency

VirtualCrypto could not let a bot touch a user's balance, so every automated flow
had to be built out of claims and commands a person pressed. A **contract** is
that missing piece: an application says which currency it wants to operate, whose,
how much, and until when; the users it names approve; and on approval their
amount is **locked** — moved out of their balance and into the contract — where
the application can spend it. A contract that everyone has approved is `active`,
and the application is told so over its webhook.

This is **not a port**. The Elixir has no contract flow: its `/contract/:id` page
is a mockup with no assigns and no endpoint behind it, and the Elm has no contract
route at all (see `docs/web-ui.md`). What is kept from it is the shape somebody
intended — a contract proposes a use of money, and the people it names approve or
refuse it — and everything else is read out of the conventions this service
already has: an application asks, someone answers, the answer is pushed to the
application's webhook, and a token kind or a scope decides who may ask.

**Its tables are replaced, not grown.** The Elixir's schema has three: `contracts`
(`intermediary_id` → applications), `contractors` (a contract's users) and
`deposit_agreements` (a user's amount per currency, deposited and executed), with
`users.contract_id` beside them. Nothing ever wrote any of them — they are empty,
and two carry foreign keys that point at the wrong tables
(`deposit_agreements.contractor_id` → contracts rather than contractors,
`currency_id` → users rather than currencies), so the sketch was never exercised
either. Since this service shares that database, migration `0004` drops them and
creates the two below, so that a database which had the sketch and one that never
did end up with the same schema.

## What a contract is

| Field | |
| --- | --- |
| the application | who may operate it, and who is told about it |
| the currency | by `unit`, the way a payment names one |
| the parties | the users it names, each with the amount that user locks |
| the receiver | optional: when named, the only Discord user the locked money may be paid to |
| the deadline | optional: absent is permanent, and permanent contracts can be withdrawn from at any time |
| the status | `pending`, `active`, or `canceled` |

Money lives in two places: `assets.amount` (a user's balance) and
`currencies.pool_amount` (a guild's issuing pool). A contract adds a third, and
the smallest one: **the parties' `remaining`**. Locking is an ordinary transfer
into it — the asset row is decremented in the same transaction that writes the
party's `remaining`, and the `assets` trigger deletes a balance that reaches
zero — so there is no separate escrow ledger to keep in step, and no balance read
anywhere has to subtract a hold.

That is deliberate: the alternative is a `locked` column that every balance read
would have to remember, and a user's balance that no longer means what it says.
Locked money is money the application may spend, and it is out of the user's
balance until it comes back.

## The lifecycle

1. **An application creates it.** `POST /api/v2/contracts`, with an application
   token carrying `vc.contract`. It names the unit, one or more parties with
   their amounts, optionally a receiver and a deadline. Nothing is locked yet and
   nothing has been agreed: this is a proposal.
2. **Each party approves or refuses.** Approval locks that party's amount and
   makes it operable **immediately** — the contract does not wait for the others.
   Refusal ends the contract and returns what the others had already locked.
3. **When every party has approved, the contract is `active`** and the
   application is told, which is the signal to start doing whatever it created
   the contract for.
4. **The application spends.** `POST /api/v2/contracts/{id}/payments` moves
   locked money to a Discord user: the receiver it names at the time, or the one
   the contract fixed. The answer carries **two remainders**, and only a payment
   that names a party makes them differ: `remaining` is what the contract as a
   whole holds, and `party_remaining` is what the party that was named has left
   (`null` when none was, because a draw across all of them is no single one's).
   **`party_discord_id`, when it is sent, says whose use the charge is for** —
   that party's remainder becomes the only thing the payment may draw on, which is
   what an application billing several people under one contract needs, because
   without it the draw is oldest-approval-first across all of them and "whose use
   was this" has no answer. And a payment whose receiver *is* the
   party it draws on is a **return** rather than a spend: it is allowed even where
   the receiver is fixed, and it is the correction an application has for a use it
   should not have billed. Nothing else about the money is the application's to do.
5. **The contract ends** when a party refuses, when a party withdraws, or —
   for a temporary contract — when its deadline passes: nothing may be spent
   after it, and the service settles the contract itself rather than waiting for
   anyone to come back for their money.

**A withdrawal ends the contract, and everyone's remainder goes home.** A
contract is the agreement of exactly the parties it names: one of them leaving
means it can never be the agreement it was written as, and leaving the others'
money locked for an application whose counterpart is gone would be holding it
for nothing. The webhook tells the application, whose proper answer is to stop
spending and, if it still wants the rest, to write a new contract.

**A temporary contract cannot be withdrawn from while it is running**, which is
what makes it temporary: the delegation is for the period, not for as long as the
party feels like it. After the deadline it is over, and withdrawal is how the
remainder comes back.

**The clock settles what it runs out on.** `vc_api::scheduler` ticks — a minute by
default, `VCRYPTO_SETTLE_INTERVAL_SECS` to change it, `0` to turn it off — and
every contract still standing whose `expires_at` has passed is settled: each
party's remainder is refunded, the contract becomes `expired` rather than
`canceled` (the deadline did it, not anyone in it), and the application is told
over its webhook, the way it is told about a decision. A tick takes a bounded
chunk of them — the ones that ran out longest ago, first — so a backlog cannot
hold the tick: what a tick does not reach is still expired and still the next
tick's work.

Settling is what a party's own withdrawal would have done, so the two race safely:
the contract row is locked first, and whichever runs second finds a contract that
is already over and does nothing. Spending is refused from the deadline onwards
whether or not the tick has run yet, so no money moves in the gap.

A deployment may also turn the clock off — a test that settles by hand does — and
then a deadline is only a comparison until somebody withdraws, which is what this
document said before the scheduler existed.

## Who may do what

| | |
| --- | --- |
| create, read, spend | the application, with an `app` token carrying `vc.contract` |
| approve, refuse, withdraw | the party, with a `user` token — the caller *is* the user named |
| read a contract | the application that wrote it, and its parties |
| anything else | 404, whether the contract exists or not |

The scope is not the authority: what lets an application spend a user's money is
that user's approval, and `vc.contract` only says the application is one that may
ask. A contract its caller is not a party to is answered as a contract that is
not there — the same nothing a wrong id is, so neither can be used to ask which
ids are real.

## Reading the numbers

Everything about the money is in the contract itself: `GET /api/v2/contracts/{id}` answers the
contract's `remaining`, and every party's `amount`, `remaining` and `status`, which is what an
application decides how much to pay out from.

**There is no balance read beside it, and there was one.** `GET /api/v2/contracts/{id}/balances`
answered each approved party's `assets.amount` — their *wallet* in the contract's currency —
while the contract is live. It is gone, for three reasons that all say the same thing: what a
party holds outside the contract is not what the contract operates on; an application can spend
the locked amount whatever the wallet says, so the read decided nothing; and a user who locked
one coin had handed the application a window into the rest of their balance, which is not what
they agreed to. The number an application needs was in the contract's own answer all along.

## Retrying a charge

A charge is the one write in this family that an `Idempotency-Key` matters for,
and the header means here what `docs/oauth2.md` and the claim endpoints mean by
it. The key belongs to **the application** — its own account is the one it is
scoped by — so two applications may use the same one without meeting.

Three rules make the key mean one thing: **the same charge, once**.

**A request that never becomes a charge does not spend it.** A body that does not
parse, or a token that is not an application's, is refused *before* the key is
claimed, so a client whose body was malformed fixes it and sends the same key
again — and that request is the charge, rather than a replay of its own typo. The
payment and issuing endpoints arrange the same two the same way, which is the one
place this port does not follow the Elixir: its plug claims the key and then hands
an unread body to the controller, so a request the controller refused left its key
claimed with nothing recorded under it.

**A charge that happened answers, whatever the answer is.** A charge that
succeeded replays as its own `201` and its own numbers, and **a charge that was
refused replays as the refusal**, because "the quota is gone" is an answer a retry
has to get rather than a second attempt at.

**A key is taken at the caller's word.** A key that already answered is answered
from the key, whatever the body of the request that carries it says: nothing here
compares the two. The specification suggests refusing a key reused for a
*different* request with `422`, and that is declined deliberately — it defines no
way to tell whether two requests are the same, so the comparison would be this
service's invention, and any invention can refuse an honest retry (a bulk list in
another order, a field this API ignores left out, an amount written the other way)
with a status no client of this API has seen. What remains is the cost, and it is
the caller's to carry: a client that reuses a key for a request that is genuinely
different is shown the first request's answer, and its second charge never
happens. Discipline about keys is the caller's half of this feature, and
`GET /api/v2/contracts/{id}/payments` is where to see what a key did.

**The claim, the charge and the answer are one commit.** The key's row is inserted
by the same transaction that performs the charge and stores the answer, so they
cannot come apart: a charge that failed rolls back with its claim (nothing moved,
and the key is the caller's to use again, rather than a failure it is stuck behind
for a week), and a process that dies mid-request takes the claim down with it
instead of leaving a row that says "in flight" about a request that is over.

Two requests with one key at the same time serialize on the key's own unique
index: the second waits inside its insert until the first commits — and then reads
the first one's answer — or until the first rolls back, and then charges itself.
That is `READ COMMITTED` behaviour, so the transaction names that level rather than
inheriting whatever a deployment set: a level that refuses the insert instead (as
`REPEATABLE READ` does) would change what a retry is answered without this
changing.

**And the wait is bounded**: past a second it is answered `409 processing`, which
tells the client to come back rather than leaving it sitting on a row a slow
request is holding. Bounding it is also why the claim cannot be committed *before*
the write, which is how the Elixir's plug did it: a claim that can outlive its
request is a key nothing can answer for. The unit the answer carries is read before
the charge as well, leaving the charge as the only step that can fail.

The response header says which of the three things happened:
`Idempotency-Status: OK` for the request that did the work, `Duplicate` for one
that read it back, `Not Requested` for a request that carried no key. A key that is being used right now answers `409 processing` and asks to be
retried — the body is the same one, and the standard `Retry-After: 1` beside it is
the number it does not carry; a key that is not the quoted string the specification
asks for is a `400` before anything is claimed. (One second is the wait that ran
out, and asking again is what the caller should do: a retry that lands after the
holder commits reads the stored answer.)

## Reading what it paid

`GET /api/v2/contracts/{id}/payments` is the statement: what the contract has paid
out, newest first. The same readers as the contract itself — the application that
wrote it and the users it names, and nobody else, with a stranger answered as a
contract that is not there.

**A row is a ledger entry rather than a charge.** A payment draws on as many
parties as it needs and writes one row per party drawn on, so a statement of a
multi-party contract has several rows for one payment; for the one-party contract
a metered application writes, the two are the same list. A row names the party the
money came out of, the amount of that slice, the receiver, and when.

The ledger does not say which contract a row belongs to on its own: the column
that does it is this service's own, added in migration `0009`, and **the rows
written before it are not in any statement** — they are byte for byte an ordinary
transfer, and a guess built out of who sent, who received and which currency would
file somebody's own payment under a contract it never belonged to. A contract's
statement says what the contract has paid since that migration, which is the
honest thing for it to say.

Both contract lists and the statement take `limit`, `next` and `on_next`, the way
the claim list does, and a page that came back exactly full carries the `link`
header that continues it. **All three answer fifty rows when the caller does not
say**, and **two hundred is the ceiling** however they ask — a list endpoint that
answers every row by default spends a caller's memory in proportion to their data,
on a request that did not ask for it, and that is what the first version of these
two did. A `limit` above the ceiling is refused (`invalid_limit`) rather than
quietly trimmed, because a caller given a smaller page than it asked for may take
that page for everything.

## The event

A decision about a contract is delivered as a **type-4** event, to the
application that wrote it:

```json
{"type": 4, "data": {
  "contract": {"id": "12", "unit": "nyan", "guild_id": "494...", "status": "active",
               "receiver_discord_id": null, "expires_at": null, "remaining": "300"},
  "parties": [{"discord_id": "100...", "amount": "100", "remaining": "100",
               "status": "approved"}]}}
```

A party approving and the last party approving are **the same event with
different state**: the first has `status: "pending"` and one more approved party,
the second has `status: "active"`. That is how the claim update already works —
the state is what the event carries — and it means an application that only cares
about "everyone is in" reads one field rather than subscribing to two events and
joining them. A refusal and a withdrawal travel the same way, with `canceled`.

The delivery is the grant decision's: the notifier reads the contract **when it
sends**, so what an application receives is what the contract is then, not what
it was when the decision was made. An application without a webhook reads
`GET /api/v2/contracts` instead, which is why the event is a ping and not the
state of record.

`subscribed_events` names the types an application wants, and `4` is a new one:
**an existing application is not sent contract events until it asks for them.**
The set it wrote was what it wanted, and a new type it never named is not it —
the alternative would be sending something new to everyone who once accepted
everything, which is the one thing an explicit set must not mean.

## Where it is answered

**`/contract list` in Discord**, and only there. A contract is between an
application and a user, so no guild is being asked anything — which is why the
command runs in a DM as well as in a guild. It shows who is asking, what the
caller's part of it is, how far the rest has come, where money may go and how long
it lasts, and offers only the answers that contract takes: 承認する and 拒否する
while it is waiting, 取り消す once it is theirs to take back.

A fixed receiver is shown as a Discord mention. Without a fixed receiver, the
screen says `制限なし`.

The application is shown as a mention of its bound Discord Bot. An application
without a binding is marked `Bot未連携`, with its self-chosen name and public
`client_id`; identical names alone must not make two applications look alike.

**Five of them, and arrows to the rest** — the count and one page are two bounded
reads, and the arrows carry page numbers. That is where this screen and the API
differ about pagination on purpose: the API pages this family with a cursor
(`next`/`on_next`, and the statement's `link` header), which is stable while rows
are inserted, and a screen cannot say "back" with one — first, previous, next and
last are numbers. `vc_core::contract` offers a read for each (`of_party`,
`open_of_party`) for that reason, and the claim screen beside this one pages the
same way, with numbers its list's API does not have either.

**There is no page for it.** The frontend is the landing page and the
verification warning, and nothing that manages anything: every operation this
service has is one a command can do, so a screen that only repeats a command is a
screen to keep in step for no reason. A contract is answerable from
Discord, and that is the whole of its surface.

## What is deliberately not here

- **Claims and issuing are not rebuilt on contracts.** The claim flow is a ported
  public contract with tests of its own, and the issuing endpoint is a guild's
  decision about its pool; neither is a user delegating to an application. The
  direction — one mechanism for "money moves when the people it concerns agree" —
  is worth having, and it is a migration of its own.
- **An application cannot change a contract** (its amounts, its deadline) or end
  it early. Amounts are what the parties agreed to, and a party can withdraw or
  wait out the deadline; an application that wants different terms writes a
  different contract. A subscription is therefore a contract per period and an
  approval per period, which is the whole of what stands where automatic renewal
  would — `docs/renewal.md` is where the questions that would have to be answered
  first are written down.
- **Four jobs run on the clock.** Settling expired contracts, deleting the rows
  whose `expires` has passed (`vc_core::purge`), refilling each pool once a day
  (`vc_core::currency::reset_pool_amount`), and re-checking the webhooks that have
  gone quiet (`crate::scheduler::reverify_webhooks`, which is `docs/known-gaps.md`'s
  business rather than a contract's). `docs/known-gaps.md` is where the Elixir's
  job list and what this service does and does not reproduce of it are written
  down.
