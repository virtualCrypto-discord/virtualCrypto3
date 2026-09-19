//! The clock: contracts whose deadline has passed are settled without anyone
//! having to come back for their money.
//!
//! What a tick does is decided by the rows it finds, so the rows are what these
//! write — the job itself is driven directly rather than waited for, and no test
//! sleeps on a timer.

mod support;

use std::sync::Arc;

use sqlx::PgPool;
use support::{
    Recorded, account_of, fake, insert_application, insert_asset, insert_currency, insert_user,
    mint_app, state_with_notifier,
};
use vc_core::contract::NewParty;

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const PARTY: i32 = 2;
const PARTY_DISCORD_ID: i64 = 100_000_000_000_000_001;
const GUILD: i64 = 900_000_000_000_000_001;

async fn fixture(pool: &PgPool) -> i64 {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, PARTY, PARTY_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, PARTY, 1, 1_000).await;

    insert_application(pool, OWNER_DISCORD_ID, "an application").await
}

/// A temporary contract, approved by its one party, and then aged past its
/// deadline — which is the state the clock is for. The clock is not something a
/// test can move, so the row is what says the deadline has passed.
async fn aged(pool: &PgPool, application: i64) -> i64 {
    let id = vc_core::contract::create(
        pool,
        application,
        "nyan",
        &[NewParty {
            discord_id: PARTY_DISCORD_ID,
            amount: 100,
        }],
        None,
        Some(3_600),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a contract");

    vc_core::contract::approve(pool, id, PARTY, time::OffsetDateTime::now_utc())
        .await
        .expect("an approval");

    sqlx::query!(
        "UPDATE contracts SET expires_at = now() - interval '1 second' WHERE id = $1",
        id
    )
    .execute(pool)
    .await
    .expect("age the contract");

    id
}

async fn balance(pool: &PgPool, account: i32) -> i64 {
    sqlx::query_scalar!(
        "SELECT amount FROM assets WHERE user_id = $1 AND currency_id = 1",
        i64::from(account)
    )
    .fetch_optional(pool)
    .await
    .expect("a balance")
    .flatten()
    .unwrap_or(0)
}

async fn status_of(pool: &PgPool, id: i64) -> String {
    sqlx::query_scalar!("SELECT status FROM contracts WHERE id = $1", id)
        .fetch_one(pool)
        .await
        .expect("the contract")
}

/// The money goes home and the application is told, which is the whole of what the
/// clock is for: a party never has to come back for a remainder.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_expired_contract_is_settled_and_told_about(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = aged(&pool, application).await;
    let recorded = Arc::new(Recorded::default());
    let state = state_with_notifier(pool.clone(), fake(), recorded.clone());

    assert_eq!(balance(&pool, PARTY).await, 900, "locked before the clock");

    vc_api::scheduler::settle_expired(&state).await;

    assert_eq!(balance(&pool, PARTY).await, 1_000, "and home after it");
    assert_eq!(status_of(&pool, id).await, "expired");
    assert_eq!(recorded.contract_decisions(), [(application, id)]);

    let contract = vc_core::contract::find(&pool, id)
        .await
        .expect("a read")
        .expect("the contract");

    assert_eq!(contract.remaining, 0);
    assert_eq!(
        contract.parties[0].remaining, 0,
        "the settlement is what emptied it"
    );
}

/// A contract whose time has not come is left alone, which is the half that makes
/// the other half safe to run every minute.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_contract_that_is_still_running_is_left_alone(pool: PgPool) {
    let application = fixture(&pool).await;

    let id = vc_core::contract::create(
        &pool,
        application,
        "nyan",
        &[NewParty {
            discord_id: PARTY_DISCORD_ID,
            amount: 100,
        }],
        None,
        Some(3_600),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a contract");

    vc_core::contract::approve(&pool, id, PARTY, time::OffsetDateTime::now_utc())
        .await
        .expect("an approval");

    let recorded = Arc::new(Recorded::default());
    let state = state_with_notifier(pool.clone(), fake(), recorded.clone());

    vc_api::scheduler::settle_expired(&state).await;

    assert_eq!(balance(&pool, PARTY).await, 900, "still locked");
    assert_eq!(status_of(&pool, id).await, "active");
    assert!(recorded.contract_decisions().is_empty());
}

/// A second tick finds nothing to do: the money moved once, and the application
/// was told once.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn settling_twice_settles_once(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = aged(&pool, application).await;
    let recorded = Arc::new(Recorded::default());
    let state = state_with_notifier(pool.clone(), fake(), recorded.clone());

    vc_api::scheduler::settle_expired(&state).await;
    vc_api::scheduler::settle_expired(&state).await;

    assert_eq!(balance(&pool, PARTY).await, 1_000, "refunded once");
    assert_eq!(recorded.contract_decisions(), [(application, id)]);
}

/// A contract nobody approved still runs out of time, and saying so is worth an
/// event: the application was waiting for answers that are not coming.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_contract_nobody_approved_is_over_too(pool: PgPool) {
    let application = fixture(&pool).await;

    let id = vc_core::contract::create(
        &pool,
        application,
        "nyan",
        &[NewParty {
            discord_id: PARTY_DISCORD_ID,
            amount: 100,
        }],
        None,
        Some(3_600),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a contract");

    sqlx::query!(
        "UPDATE contracts SET expires_at = now() - interval '1 second' WHERE id = $1",
        id
    )
    .execute(&pool)
    .await
    .expect("age the contract");

    let recorded = Arc::new(Recorded::default());
    let state = state_with_notifier(pool.clone(), fake(), recorded.clone());

    vc_api::scheduler::settle_expired(&state).await;

    assert_eq!(status_of(&pool, id).await, "expired");
    assert_eq!(balance(&pool, PARTY).await, 1_000, "nothing had moved");
    assert_eq!(recorded.contract_decisions(), [(application, id)]);
}

/// A party who got their remainder back from the clock cannot take it back twice:
/// the contract is over, and the endpoint says so.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_settled_contract_cannot_be_withdrawn_from(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = aged(&pool, application).await;
    let state = state_with_notifier(pool.clone(), fake(), Arc::new(Recorded::default()));

    vc_api::scheduler::settle_expired(&state).await;

    let refused = vc_core::contract::withdraw(&pool, id, PARTY, time::OffsetDateTime::now_utc())
        .await
        .expect_err("a refusal");

    assert!(matches!(
        refused,
        vc_core::contract::ContractError::InvalidStatus
    ));
    assert_eq!(
        balance(&pool, PARTY).await,
        1_000,
        "and nothing moved twice"
    );
}

/// The application's token is not what settles anything, but the event still
/// belongs to it: a check that the clock needs no caller at all.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_clock_needs_nobody_awake(pool: PgPool) {
    let application = fixture(&pool).await;
    let account = account_of(&pool, application).await;
    let _token = mint_app(&pool, account, &["vc.contract"]).await;
    let id = aged(&pool, application).await;

    let recorded = Arc::new(Recorded::default());
    let state = state_with_notifier(pool.clone(), fake(), recorded.clone());

    vc_api::scheduler::settle_expired(&state).await;

    assert_eq!(
        recorded.contract_decisions(),
        [(application, id)],
        "nobody called an endpoint"
    );
}
