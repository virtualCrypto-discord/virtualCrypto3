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
   the contract fixed. Nothing else about the money is the application's to do.
5. **The contract ends** when a party refuses, when a party withdraws, or —
   for a temporary contract — when its deadline passes, after which nothing may
   be spent and each party may take back what is left.

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

**Nothing runs on a clock.** This service has no scheduler (see
`docs/known-gaps.md`), so a deadline is a comparison rather than an event:
spending is refused once `expires_at` has passed, and the refund happens when a
party withdraws. A party who never withdraws leaves their remainder locked, which
is the user's own to fix — the contract says when it ended, and the web and
Discord both say what is left.

## Who may do what

| | |
| --- | --- |
| create, read, spend, read balances | the application, with an `app` token carrying `vc.contract` |
| approve, refuse, withdraw | the party, with a `user` token — the caller *is* the user named |
| read a contract | the application that wrote it, and its parties |
| anything else | 404, whether the contract exists or not |

The scope is not the authority: what lets an application spend a user's money is
that user's approval, and `vc.contract` only says the application is one that may
ask. A contract its caller is not a party to is answered as a contract that is
not there — the same nothing a wrong id is, so neither can be used to ask which
ids are real.

## Reading balances

A party that approved gave the application the right to *see* what it is
operating on: `GET /api/v2/contracts/{id}/balances` answers the parties' balances
in the contract's currency, and nothing else — not their other currencies, not
the guild's pool. An application that cannot read a balance cannot decide how
much to pay out, which is why this travels with the operation authority rather
than being a separate permission to ask for.

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

Two screens draw the same set, in the same words: **`/contracts`** in the SPA,
for a user with a browser, and **`/contract list`** in Discord — one of the
commands that runs in a DM as well as in a guild, because a contract is between an
application and a user and no guild is being asked anything. Both show who is
asking, what the caller's part is, how far the rest has come and how long it
lasts, and both offer only the answers that contract takes: 承認する and 拒否する
while it is waiting, 取り消す once it is theirs to take back.

## What is deliberately not here

- **Claims and issuing are not rebuilt on contracts.** The claim flow is a ported
  public contract with tests of its own, and the issuing endpoint is a guild's
  decision about its pool; neither is a user delegating to an application. The
  direction — one mechanism for "money moves when the people it concerns agree" —
  is worth having, and it is a migration of its own.
- **An application cannot change a contract** (its amounts, its deadline) or end
  it early. Amounts are what the parties agreed to, and a party can withdraw or
  wait out the deadline; an application that wants different terms writes a
  different contract.
- **Nothing refunds on a timer**, for the reason above: no scheduler.
