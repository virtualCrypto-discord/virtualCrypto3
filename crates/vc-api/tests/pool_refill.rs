//! The pools get a day's allowance, which is the Elixir's `@daily` job.

mod support;

use sqlx::PgPool;
use support::{fake, insert_asset, insert_currency, insert_user, state};

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_pool_grows_by_a_days_allowance_and_stops_at_the_ceiling(pool: PgPool) {
    insert_user(&pool, 1, 100).await;
    insert_currency(&pool, 1, "nyan", "nyan", 1, 5).await;
    insert_asset(&pool, 1, 1, 1_000).await;

    let state = state(pool.clone(), fake());

    // With 1000 supplied: an allowance of (1000 + 199) / 200 = 6, and a ceiling of
    // (7000 + 199) / 200 = 36.
    vc_api::scheduler::refill_pools(&state).await;

    let pool_amount = || async {
        sqlx::query_scalar!("SELECT pool_amount FROM currencies WHERE id = 1")
            .fetch_one(&pool)
            .await
            .expect("the pool")
    };

    assert_eq!(pool_amount().await, Some(11), "5 + 6");

    for _ in 0..10 {
        vc_api::scheduler::refill_pools(&state).await;
    }

    assert_eq!(pool_amount().await, Some(36), "the ceiling is 3.5%");
}

/// A currency nobody holds is not in the supply and is left alone, which is what
/// the Elixir's `GROUP BY currency_id` over `assets` decides.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_pool_nobody_holds_is_left_alone(pool: PgPool) {
    insert_user(&pool, 1, 100).await;
    insert_currency(&pool, 1, "nyan", "nyan", 1, 5).await;

    let state = state(pool.clone(), fake());

    vc_api::scheduler::refill_pools(&state).await;

    let pool_amount = sqlx::query_scalar!("SELECT pool_amount FROM currencies WHERE id = 1")
        .fetch_one(&pool)
        .await
        .expect("the pool");

    assert_eq!(pool_amount, Some(5));
}
