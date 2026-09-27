//! Discord claim creation must share the currency-deletion lock order even
//! without the REST handler's resource authorization lock.

mod support;

use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use support::*;
use vc_core::claim::{self, CreateError};

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
    let mut blocker = pool.begin().await.unwrap();
    let create = || {
        let pool = pool.clone();
        tokio::spawn(async move {
            claim::create(
                &pool,
                1,
                MONEY_USER2,
                "n",
                10,
                Some(json!({"invoice": "one"})),
            )
            .await
        })
    };
    let delete = || {
        let pool = pool.clone();
        tokio::spawn(async move { vc_core::currency::delete(&pool, MONEY_GUILD, "delete n").await })
    };
    let (creation, deletion) = if deletion_first {
        sqlx::query("SELECT id FROM assets WHERE currency_id=$1 FOR UPDATE")
            .bind(money.currency)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let deletion = delete();
        wait_for_lock(&pool, &["DELETE FROM assets%"]).await;
        let creation = create();
        // Before the fix, the insertion waits at its foreign key and later
        // fails with 23503. The protected lookup returns NotFoundCurrency.
        wait_for_lock(
            &pool,
            &[
                "SELECT id FROM currencies%FOR KEY SHARE%",
                "INSERT INTO claims%",
            ],
        )
        .await;
        (creation, deletion)
    } else {
        sqlx::query("SELECT id FROM users WHERE id=2 FOR UPDATE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let creation = create();
        wait_for_lock(&pool, &["SELECT id, discord_id%FOR KEY SHARE%"]).await;
        let mut deletion = delete();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut deletion)
                .await
                .is_err(),
            "deletion must wait for the claim's transaction"
        );
        (creation, deletion)
    };
    blocker.rollback().await.unwrap();
    let (created, deleted) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(creation, deletion)
    })
    .await
    .expect("both operations must finish");
    let created = created.unwrap();
    if deletion_first {
        assert!(
            matches!(created, Err(CreateError::NotFoundCurrency)),
            "{created:?}"
        );
    } else {
        created.expect("creation must commit before deletion");
    }
    assert_eq!(
        deleted.unwrap().unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM claims")
        .fetch_one(&pool)
        .await
        .unwrap();
    let metadata: i64 = sqlx::query_scalar("SELECT count(*) FROM claim_metadata")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((claims, metadata), (0, 0));
    assert_eq!(
        get_amount(&pool, MONEY_USER2, money.currency2).await,
        200_000
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deletion_before_discord_claim_creation_reports_missing_currency(pool: PgPool) {
    competing(pool, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn discord_claim_creation_finishes_before_currency_deletion(pool: PgPool) {
    competing(pool, false).await;
}
