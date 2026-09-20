# Renewal: what a subscription would need, and what is not answered yet

**Nothing in this document is implemented.** It is the design pass a recurring
contract needs before any of it is written, and it is here because a subscription
is the first thing anybody asks a metered application for. What exists today is a
contract per period and an approval per period (`docs/contracts.md`), which is a
defensible answer that should be given up deliberately rather than by accident.

## What stands today

A temporary contract is a delegation for a period: the parties lock their amounts
by approving, the application may spend what is left while the period runs, and
when it runs out nothing more may be spent and the service settles it — every
party's remainder goes home, `expired`, and the application is told over its
webhook. A party cannot withdraw from a temporary contract while it runs, because
the period is what they agreed to, and neither can the application end one early.

So a subscription exists in the shape of *one contract per period*, and the
renewal is a new contract with a new approval. What that costs the parties is an
approval a month; what it costs the application is a webhook that says "the period
ended" and a screen the subscriber may or may not answer. Nothing is wrong with
it, and it has one property worth keeping sight of: **nobody is ever charged for a
period they did not agree to.**

## The questions a renewal has to answer

1. **What does one approval cover?** Only the period that names it, or a standing
   permission to be locked again — and if standing, against what bound: an amount
   per period, a total across periods, a number of periods, or all three.
2. **When is the next period locked?** At the deadline, before it, or only when
   the application asks — and if at the deadline, is the application guaranteed a
   period it has not used yet.
3. **What if the balance is not there?** A party who spent their currency between
   periods is a party the renewal cannot lock. Skip the period (and say so), take
   whatever is left, or take nothing and let the subscription lapse.
4. **Can it be stopped, and by whom?** Today a running period cannot be ended by
   either side, which is the property that makes it a period. A renewal has to say
   whether the next period can be refused in advance, and what refusing costs.
5. **What is the money between periods?** A contract holds the parties'
   remainders; a renewal either extends one (and the remainder is simply what is
   left) or writes a new one (and the money has to be locked again, by an approval
   or by the standing permission). The two differ in what a party can see.
6. **What is the application told?** The type-4 event carries a decision. A
   renewal that needs no decision is an event with none behind it, which is a new
   kind of thing for the notification contract rather than another `type` value.
7. **Is the clock the right engine?** `expires_at` and `settle` exist. A renewal
   is a settle that locks instead of refunding, which is a different job on the
   same clock — and a job that moves money needs the same care the settling one
   has about racing a party's own decision.

## The shapes that could carry it

- **A contract that renews itself.** A period length and a per-period cap on the
  contract, and an approval that says "and again, up to this much, until I say
  otherwise". Smallest change to the tables; hardest to explain, because one row
  then holds several periods' worth of agreement and the parties' `remaining` has
  to mean what it means now.
- **A series of contracts.** What exists, with the application writing the next
  one and the party approving it. No new mechanism at all, and the honest answer
  today. It is what a subscriber who does not want to be charged silently should
  prefer.
- **A standing permission beside contracts.** A row of its own — a mandate: who,
  which unit, how much per period, until when — and the contract per period
  created from it without an approval. The cleanest model of the three, and the
  largest change: a new consent to design, a new screen to answer it on, and a new
  thing for an application to be told about.

Whichever is chosen, what does *not* change is the invariant the feature exists
under: money is never locked without an agreement that says so, a party's cap can
never be exceeded by a renewal, and a period that fails to renew leaves nothing
half-locked. Anything that cannot hold those three is not a design of this
feature.
