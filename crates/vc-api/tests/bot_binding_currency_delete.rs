//! A bot can retain currency references after spending its entire balance.
//! Binding and deletion must serialize those references as well as assets.

mod support;

use sqlx::PgPool;
use std::time::Duration;
use support::*;

const BOT: i64 = 500_000_000_000_000_001;

async fn wait_for_lock(pool: &PgPool, patterns: &[&str]) {
    let patterns: Vec<String> = patterns.iter().map(|pattern| pattern.to_string()).collect();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                 WHERE datname=current_database() AND wait_event_type='Lock'
                   AND query LIKE ANY($1))",
            )
            .bind(&patterns)
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("operation should reach its lock wait");
}

async fn competing(pool: PgPool, deletion_first: bool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "currency deletion").await;
    let destination = account_of(&pool, application).await;
    vc_core::payment::pay(&pool, 2, BOT, "n", 1).await.unwrap();
    let source = vc_core::user::find_by_discord_id(&pool, BOT)
        .await
        .unwrap()
        .unwrap()
        .id;
    vc_core::payment::pay(&pool, source, MONEY_USER2, "n", 1)
        .await
        .unwrap();
    vc_core::claim::create(&pool, 1, BOT, "n", 10, None)
        .await
        .unwrap();
    assert_eq!(get_amount(&pool, BOT, money.currency).await, 0);
    let assets: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE user_id IN ($1, $2)")
        .bind(i64::from(source))
        .bind(i64::from(destination))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(assets, 0, "only history and claims retain this currency");

    sqlx::raw_sql(
        "CREATE FUNCTION pause_history_change() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(937272); RETURN OLD; END $$;",
    )
    .execute(&pool)
    .await
    .unwrap();
    let trigger = if deletion_first {
        "CREATE TRIGGER pause_history_change BEFORE DELETE ON currency_payment_histories
         FOR EACH ROW EXECUTE FUNCTION pause_history_change()"
    } else {
        "CREATE TRIGGER pause_history_change BEFORE UPDATE ON currency_payment_histories
         FOR EACH STATEMENT EXECUTE FUNCTION pause_history_change()"
    };
    sqlx::query(trigger).execute(&pool).await.unwrap();
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(937272)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let bind = || {
        let pool = pool.clone();
        tokio::spawn(async move { vc_core::user::bind_bot(&pool, destination, BOT).await })
    };
    let delete = || {
        let pool = pool.clone();
        tokio::spawn(async move { vc_core::currency::delete(&pool, MONEY_GUILD, "delete n").await })
    };
    let (binding, deletion) = if deletion_first {
        let deletion = delete();
        wait_for_lock(&pool, &["DELETE FROM currency_payment_histories%"]).await;
        let mut binding = bind();
        // The binding retries its NOWAIT locks until the deletion commits.
        // Previously it held the claim and waited for deletion's history row.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut binding)
                .await
                .is_err()
        );
        (binding, deletion)
    } else {
        let binding = bind();
        wait_for_lock(&pool, &["UPDATE currency_payment_histories%"]).await;
        let deletion = delete();
        wait_for_lock(
            &pool,
            &[
                "SELECT id, unit, inserted_at FROM currencies%",
                "DELETE FROM claims%",
            ],
        )
        .await;
        (binding, deletion)
    };
    blocker.rollback().await.unwrap();
    let (bound, deleted) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(binding, deletion)
    })
    .await
    .expect("both operations must finish");
    bound.unwrap().expect("binding must not deadlock");
    assert_eq!(
        deleted.unwrap().unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    let bound = vc_core::user::find_by_discord_id(&pool, BOT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound.id, destination);
    assert!(
        vc_core::user::find_by_id(&pool, source)
            .await
            .unwrap()
            .is_none()
    );
    for (table, query) in [
        ("currencies", "SELECT count(*) FROM currencies WHERE id=$1"),
        ("assets", "SELECT count(*) FROM assets WHERE currency_id=$1"),
        ("claims", "SELECT count(*) FROM claims WHERE currency_id=$1"),
        (
            "currency_payment_histories",
            "SELECT count(*) FROM currency_payment_histories WHERE currency_id=$1",
        ),
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .bind(money.currency)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "deleted currency must have no remaining {table}");
    }
    assert_eq!(
        get_amount(&pool, MONEY_USER2, money.currency2).await,
        200_000
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deletion_before_binding_a_bot_with_history_and_no_balance(pool: PgPool) {
    competing(pool, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_a_bot_with_history_and_no_balance_before_deletion(pool: PgPool) {
    competing(pool, false).await;
}
