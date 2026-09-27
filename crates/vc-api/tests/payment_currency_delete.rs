mod support;

use sqlx::PgPool;
use std::time::Duration;
use support::*;

async fn wait(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)",
            )
            .bind(pattern)
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

async fn competing(pool: PgPool, bulk: bool, deletion_first: bool) {
    setup_money(&pool).await;
    let mut blocker = pool.begin().await.unwrap();
    // Pause payment at the receiver upsert, or deletion at its asset removal.
    sqlx::query("SELECT * FROM assets WHERE user_id=2 AND currency_id=1 FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pay = || {
        let p = pool.clone();
        tokio::spawn(async move {
            if bulk {
                vc_core::payment::pay_bulk(
                    &p,
                    1,
                    &[vc_core::payment::BulkPayment {
                        unit: "n".into(),
                        receiver_discord_id: MONEY_USER2,
                        amount: 100,
                    }],
                )
                .await
            } else {
                vc_core::payment::pay(&p, 1, MONEY_USER2, "n", 100).await
            }
        })
    };
    let delete = || {
        let p = pool.clone();
        tokio::spawn(async move { vc_core::currency::delete(&p, MONEY_GUILD, "delete n").await })
    };
    let (payment, deletion) = if deletion_first {
        let deletion = delete();
        wait(&pool, "DELETE FROM assets%").await;
        let payment = pay();
        wait(&pool, "SELECT id%FROM currencies%FOR KEY SHARE%").await;
        (payment, deletion)
    } else {
        let payment = pay();
        wait(&pool, "INSERT INTO assets%").await;
        let deletion = delete();
        wait(&pool, "SELECT id, unit, inserted_at FROM currencies%").await;
        (payment, deletion)
    };
    blocker.rollback().await.unwrap();
    let (payment, deletion) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(payment, deletion)
    })
    .await
    .unwrap();
    let payment = payment.unwrap();
    if deletion_first {
        assert!(
            matches!(payment, Err(vc_core::payment::PayError::NotFoundCurrency)),
            "{payment:?}"
        );
    } else {
        payment.expect("payment must commit without deadlocking");
    }
    assert_eq!(
        deletion.unwrap().unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    let assets: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE currency_id=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(assets, 0);
    let histories: i64 =
        sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories WHERE currency_id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(histories, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn single_payment_before_deletion(pool: PgPool) {
    competing(pool, false, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bulk_payment_before_deletion(pool: PgPool) {
    competing(pool, true, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deletion_before_single_payment(pool: PgPool) {
    competing(pool, false, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deletion_before_bulk_payment(pool: PgPool) {
    competing(pool, true, true).await;
}
