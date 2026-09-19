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

/// One tick's work, first half: every contract whose deadline has passed is
/// settled, and the application that wrote it is told.
///
/// One transaction per contract rather than one for all of them: they are
/// independent agreements, and a contract that cannot be settled — a database
/// error, a row somebody else is deciding about at this moment — must not hold up
/// the rest. `settle` answers whether it did anything, which is also what keeps a
/// second tick from refunding twice.
pub async fn settle_expired(state: &AppState) {
    let now = time::OffsetDateTime::now_utc();

    let ids = match vc_core::contract::expired(state.pool(), now).await {
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

    match vc_core::job::claim_day(state.pool(), "reset_pool_amount", today).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            tracing::warn!(?error, "the pool refill could not claim its day");
            return;
        }
    }

    match vc_core::currency::reset_pool_amount(state.pool()).await {
        Ok(0) => {}
        Ok(updated) => tracing::info!(updated, "the pools were refilled"),
        Err(error) => tracing::warn!(
            ?error,
            "the day was claimed and the pools were not refilled"
        ),
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
    }
}
