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
/// `@daily` in the Elixir and here: the day the tick is in is held in memory, so
/// the first tick of a new UTC day refills and the rest of that day's ticks do
/// nothing. A service that restarts later the same day refills again, because
/// nothing in the database says the day was already paid — the Elixir's schedule
/// lives in its own memory too, and a marker column would be a schema this service
/// shares with it and must not add on its own.
///
/// The day is a value rather than a clock reading passed in, so a test can ask for
/// a day without waiting for one.
pub async fn refill_pools(state: &AppState) {
    match vc_core::currency::reset_pool_amount(state.pool()).await {
        Ok(0) => {}
        Ok(updated) => tracing::info!(updated, "the pools were refilled"),
        Err(error) => tracing::warn!(?error, "the pools could not be refilled"),
    }
}

/// Whether this tick is the first of a new day, and the day to remember.
fn is_a_new_day(last: Option<time::Date>, today: time::Date) -> bool {
    last != Some(today)
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
    let mut day = None;

    loop {
        ticker.tick().await;
        settle_expired(&state).await;
        purge_expired(&state).await;

        let today = time::OffsetDateTime::now_utc().date();

        if is_a_new_day(day, today) {
            refill_pools(&state).await;
            day = Some(today);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first tick of a day refills and the rest of its ticks do not, which is
    /// the whole of what keeps a daily allowance daily.
    #[test]
    fn a_day_is_new_once() {
        let first = time::macros::date!(2026 - 09 - 19);
        let next = time::macros::date!(2026 - 09 - 20);

        assert!(is_a_new_day(None, first), "nothing has run yet");
        assert!(!is_a_new_day(Some(first), first), "the same day again");
        assert!(is_a_new_day(Some(first), next), "and the next one");
    }
}
