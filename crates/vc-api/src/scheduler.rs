//! The clock this service did not have.
//!
//! Until now nothing here ran on a timer, and for contracts that was a real limit
//! rather than a style: one that ran out of time kept its parties' money locked
//! until somebody came back to take it, which `docs/contracts.md` said out loud.
//! This is the task that settles them, so the deadline is something the service
//! acts on rather than something a user has to notice.
//!
//! **The work is a function, and the loop only calls it.** `settle_expired` is
//! what a test drives, and no test waits on a timer: what a tick does is decided
//! by the rows it finds, so the rows are what a test writes.

use std::time::Duration;

use crate::notification::{Handshake, check_webhook};
use crate::state::AppState;

/// How often the clock ticks, unless a deployment says otherwise. A minute is
/// short enough that a party never waits noticeably for money that is theirs and
/// long enough that an idle service does one query a minute.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);

/// The interval a deployment asked for, from `VCRYPTO_SETTLE_INTERVAL_SECS`.
///
/// Zero turns the clock off, which is what a test that drives the work itself
/// wants — and what a deployment that would rather settle by hand can say.
pub fn interval() -> Duration {
    std::env::var("VCRYPTO_SETTLE_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_INTERVAL)
}

/// How many expired contracts one tick settles.
///
/// The clock is not the only thing that bounds a tick, for the reason the rate
/// limiters are bounded rather than trusted: a backlog is what a minute of downtime
/// leaves behind, and settling tens of thousands of contracts in one tick would
/// hold the tick — and the four jobs behind it — for as long as it took to drain.
/// What a tick can do is bounded instead, and the rest of the backlog is what the
/// next tick is for: 500 a minute drains more than a deployment expires, and a
/// backlog of them is gone inside an hour.
///
/// 500 is also what the read behind it costs. `contract::expired` reads the oldest
/// deadlines first off `contracts_unsettled_expiry_index`, so the pass is one index
/// scan that stops after this many rows — measured at 0.2 ms for the plan that
/// returns it — while a pass without the bound reads, fetches and sorts every
/// contract that is over.
const EXPIRED_PER_PASS: i64 = 500;

/// One tick's work, first half: the expired contracts that ran out longest ago are
/// settled, and the application that wrote each is told.
///
/// One transaction per contract rather than one for all of them: they are
/// independent agreements, and a contract that cannot be settled — a database
/// error, a row somebody else is deciding about at this moment — must not hold up
/// the rest. `settle` answers whether it did anything, which is also what keeps a
/// second tick from refunding twice.
///
/// A tick takes `EXPIRED_PER_PASS` of them rather than all of them, so the work
/// here is bounded by the tick and not by the backlog: whatever is left is still
/// expired and still due the next time round.
pub async fn settle_expired(state: &AppState) {
    let now = time::OffsetDateTime::now_utc();

    let ids = match vc_core::contract::expired(state.pool(), now, EXPIRED_PER_PASS).await {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!(?error, "the expired contracts could not be read");
            return;
        }
    };

    for contract_id in ids {
        match vc_core::contract::settle(state.pool(), contract_id, now).await {
            Ok(true) => match vc_core::contract::find(state.pool(), contract_id).await {
                Ok(Some(contract)) => state
                    .notifier()
                    .notify_contract_decided(contract.application_id, contract_id),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(contract_id, ?error, "a settled contract could not be read");
                }
            },
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(contract_id, ?error, "a contract could not be settled");
            }
        }
    }
}

/// The pools get a day's allowance, once a day.
///
/// `@daily` in the Elixir and here. Which day it last ran on is a row
/// (`vc_core::job::claim_day`), because this loop ticks every minute and
/// `reset_pool_amount` is an increment with no memory: a day kept in a variable
/// would be paid a second time by the next deploy, which is an allowance no day
/// asked for. The claim is a statement a minute against a one-row table, which is
/// what it costs not to remember the same thing in two places — and what lets two
/// schedulers behind one database agree on the day.
pub async fn refill_pools(state: &AppState) {
    let today = time::OffsetDateTime::now_utc().date();

    let result: Result<u64, sqlx::Error> = async {
        let mut tx = state.pool().begin().await?;
        if !vc_core::job::claim_day(&mut tx, "reset_pool_amount", today).await? {
            tx.rollback().await?;
            return Ok(0);
        }
        let updated = vc_core::currency::reset_pool_amount(&mut tx).await?;
        tx.commit().await?;
        Ok(updated)
    }
    .await;

    match result {
        Ok(0) => {}
        Ok(updated) => tracing::info!(updated, "the pools were refilled"),
        Err(error) => tracing::warn!(?error, "the pool refill failed; it will retry next tick"),
    }
}

/// The rows whose time is up are deleted, so the tables do not grow with things
/// nobody can use again — an expired token, an idempotency key whose week is over,
/// an authorization code nobody redeemed.
pub async fn purge_expired(state: &AppState) {
    let now = time::OffsetDateTime::now_utc();
    let now = time::PrimitiveDateTime::new(now.date(), now.time());

    match vc_core::purge::expired(state.pool(), now).await {
        Ok(0) => {}
        Ok(deleted) => tracing::info!(deleted, "expired rows were purged"),
        Err(error) => tracing::warn!(?error, "the expired rows could not be purged"),
    }
}

/// How many webhooks one pass re-checks. Two requests each, to strangers, so the
/// batch is what keeps a pass short rather than the interval being the only bound.
const WEBHOOKS_PER_PASS: i64 = 10;

/// How long one application has to answer the handshake before it counts as
/// silent. Nothing else bounds it: `Direct` sends through a plain `reqwest`
/// client, which waits forever by default, and a webhook that accepts a connection
/// and then says nothing is exactly the case this job exists to find.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The webhooks that are due to be re-checked.
///
/// The handshake runs at registration and at an edit, and an application that
/// passed once and then stopped answering is one nobody hears about: deliveries
/// are fire-and-forget, so nothing about sending one says whether it landed. This
/// is what turns "it verified once" into something that is still true. What makes
/// a row due is its `applications.next_reverify_at`, which
/// `record_webhook_verification` moves a week out after each check (`0016`).
///
/// **It does not charge the handshake limiter.** That budget belongs to the
/// caller a handshake is made for — one per three seconds, twenty an hour, fifty a
/// day per requester — and a job re-checking a fleet would spend in a minute what
/// a fleet of callers would take all day to spend, then be refused by its own
/// limiter. What bounds this pass is the batch and the timeout instead, neither of
/// which is a caller's.
///
/// A failure is recorded and logged, and that is all: the webhook stays, and
/// deliveries keep going. What a delivery carries is a decision about somebody's
/// money, and a transient outage is not a reason to stop telling an application
/// about it.
pub async fn reverify_webhooks(state: &AppState) {
    let now = time::OffsetDateTime::now_utc();
    let at = time::PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let webhooks =
        match vc_core::application::stale_webhooks(state.pool(), at, WEBHOOKS_PER_PASS).await {
            Ok(webhooks) => webhooks,
            Err(error) => {
                tracing::warn!(?error, "the webhooks to re-check could not be read");
                return;
            }
        };

    for webhook in webhooks {
        let Ok(private_key) = <[u8; 32]>::try_from(webhook.private_key.as_slice()) else {
            tracing::warn!(
                application_id = webhook.id,
                "the application's private key is not 32 bytes"
            );
            continue;
        };

        let handshake = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            check_webhook(state, &webhook.webhook_url, &private_key),
        )
        .await
        .map(|(handshake, _)| handshake)
        .unwrap_or(Handshake::Unreachable);

        let passed = handshake == Handshake::Passed;

        if !passed {
            tracing::warn!(
                application_id = webhook.id,
                ?handshake,
                "an application's webhook did not answer the handshake"
            );
        }

        if let Err(error) =
            vc_core::application::record_webhook_verification(state.pool(), webhook.id, passed, at)
                .await
        {
            tracing::warn!(
                ?error,
                application_id = webhook.id,
                "the handshake's outcome could not be written"
            );
        }
    }
}

/// The loop: however long the deployment asked for, settle and wait again.
///
/// The first tick is immediate, so a service that was down while a deadline passed
/// settles it as soon as it is up rather than a minute later.
pub async fn run(state: AppState, every: Duration) {
    if every.is_zero() {
        tracing::info!("the contract clock is off (VCRYPTO_SETTLE_INTERVAL_SECS=0)");
        return;
    }

    let mut ticker = tokio::time::interval(every);

    loop {
        ticker.tick().await;
        settle_expired(&state).await;
        purge_expired(&state).await;
        refill_pools(&state).await;

        // The one job that is not awaited, and for a reason the other three do
        // not have: it makes requests to strangers, and no webhook that never
        // answers may hold up the tick that settles contracts — their money is
        // waiting on a deadline rather than on an application's uptime.
        let state = state.clone();
        tokio::spawn(async move { reverify_webhooks(&state).await });
    }
}
