//! The pools get a day's allowance, which is the Elixir's `@daily` job.
//!
//! `reset_pool_amount` is the Elixir's own SQL, so what these lock is its
//! behaviour: a day's worth of the supply rounded into the column, a ceiling of
//! seven days' worth, nothing for a currency nobody holds — and a day, because the
//! job ticks every minute and the SQL has no memory of its own.

mod support;

use sqlx::PgPool;
use support::{fake, insert_asset, insert_currency, insert_user, state};

/// A day later as far as `job_runs` is concerned: the row is the only thing that
/// knows when the refill last ran, so it is what a test moves.
async fn a_day_passes(pool: &PgPool) {
    sqlx::query("UPDATE job_runs SET ran_on = ran_on - 1")
        .execute(pool)
        .await
        .expect("a day passes");
}

async fn pool_amount(pool: &PgPool) -> Option<i64> {
    sqlx::query_scalar!("SELECT pool_amount FROM currencies WHERE id = 1")
        .fetch_one(pool)
        .await
        .expect("the pool")
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_pool_grows_by_a_days_allowance_and_stops_at_the_ceiling(pool: PgPool) {
    insert_user(&pool, 1, 100).await;
    insert_currency(&pool, 1, "nyan", "nyan", 1, 5).await;
    insert_asset(&pool, 1, 1, 1_000).await;

    let state = state(pool.clone(), fake());

    // With 1000 supplied: an allowance of (1000 + 199) / 200 = 6, and a ceiling of
    // (7000 + 199) / 200 = 36.
    vc_api::scheduler::refill_pools(&state).await;

    assert_eq!(pool_amount(&pool).await, Some(11), "5 + 6");

    for _ in 0..10 {
        a_day_passes(&pool).await;
        vc_api::scheduler::refill_pools(&state).await;
    }

    assert_eq!(pool_amount(&pool).await, Some(36), "the ceiling is 3.5%");
}

/// A currency nobody holds is not in the supply and is left alone, which is what
/// the Elixir's `GROUP BY currency_id` over `assets` decides.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_pool_nobody_holds_is_left_alone(pool: PgPool) {
    insert_user(&pool, 1, 100).await;
    insert_currency(&pool, 1, "nyan", "nyan", 1, 5).await;

    let state = state(pool.clone(), fake());

    vc_api::scheduler::refill_pools(&state).await;

    assert_eq!(pool_amount(&pool).await, Some(5));
}

/// A day, and only a day. The tick is a minute and `reset_pool_amount` adds a
/// day's allowance every time it is called, so the row is the whole of what keeps
/// one day from being paid twice — including by a restart, which a day held in
/// memory could not survive.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_day_is_claimed_once(pool: PgPool) {
    insert_user(&pool, 1, 100).await;
    insert_currency(&pool, 1, "nyan", "nyan", 1, 5).await;
    insert_asset(&pool, 1, 1, 1_000).await;

    let state = state(pool.clone(), fake());

    vc_api::scheduler::refill_pools(&state).await;
    assert_eq!(pool_amount(&pool).await, Some(11));

    vc_api::scheduler::refill_pools(&state).await;
    assert_eq!(pool_amount(&pool).await, Some(11), "the same day again");

    a_day_passes(&pool).await;
    vc_api::scheduler::refill_pools(&state).await;
    assert_eq!(pool_amount(&pool).await, Some(17), "and the next day's");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_refill_can_retry_that_day(pool: PgPool) {
    insert_user(&pool, 1, 100).await;
    insert_currency(&pool, 1, "nyan", "nyan", 1, 5).await;
    insert_asset(&pool, 1, 1, 1000).await;
    sqlx::query(
        "ALTER TABLE currencies ADD CONSTRAINT injected_refill_failure CHECK (pool_amount <= 5)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app_state = state(pool.clone(), fake());
    vc_api::scheduler::refill_pools(&app_state).await;
    assert_eq!(pool_amount(&pool).await, Some(5));
    sqlx::query("ALTER TABLE currencies DROP CONSTRAINT injected_refill_failure")
        .execute(&pool)
        .await
        .unwrap();
    vc_api::scheduler::refill_pools(&app_state).await;
    assert_eq!(
        pool_amount(&pool).await,
        Some(11),
        "failure must not consume the daily refill"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn concurrent_refills_pay_only_once(pool: PgPool) {
    insert_user(&pool, 1, 100).await;
    insert_currency(&pool, 1, "nyan", "nyan", 1, 5).await;
    insert_asset(&pool, 1, 1, 1000).await;
    let state = state(pool.clone(), fake());
    tokio::join!(
        vc_api::scheduler::refill_pools(&state),
        vc_api::scheduler::refill_pools(&state),
        vc_api::scheduler::refill_pools(&state),
    );
    assert_eq!(pool_amount(&pool).await, Some(11));
}
