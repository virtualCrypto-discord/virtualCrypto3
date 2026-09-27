mod support;

use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use support::*;
use vc_core::claim::{self, PartialClaim, Transition};
use vc_core::notification::NoopNotifier;

async fn wait_for_lock(pool: &PgPool, patterns: &[&str]) {
    let patterns: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE ANY($1))",
            ).bind(&patterns).fetch_one(pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("operation should reach its lock wait");
}

async fn competing(pool: PgPool, bulk: bool) {
    setup_money(&pool).await;
    // No payer metadata yet: its first INSERT must check the claim foreign key.
    let id = claim::create(&pool, 1, MONEY_USER2, "n", 100, None)
        .await
        .unwrap();
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=1 FOR NO KEY UPDATE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let p = pool.clone();
    let approval = tokio::spawn(async move {
        if bulk {
            claim::update_claims(
                &p,
                &NoopNotifier,
                2,
                &[PartialClaim {
                    id,
                    status: Some("approved".into()),
                    metadata: Some(json!({"approval":"done"})),
                }],
            )
            .await
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
        } else {
            claim::transition(
                &p,
                &NoopNotifier,
                2,
                id,
                Transition::Approved,
                Some(json!({"approval":"done"})),
            )
            .await
            .map_err(|e| format!("{e:?}"))
        }
    });
    // Approval holds the claim and waits for a transfer participant.
    wait_for_lock(&pool, &["SELECT id FROM users WHERE id = ANY%"]).await;
    let p = pool.clone();
    let metadata =
        tokio::spawn(
            async move { claim::set_metadata(&p, 2, id, Some(json!({"note":"test"}))).await },
        );
    // Before the fix this waits inside the metadata INSERT's foreign-key
    // check; afterwards it waits on the claim before touching metadata.
    wait_for_lock(
        &pool,
        &[
            "INSERT INTO claim_metadata%",
            "SELECT c.status::text AS status%",
        ],
    )
    .await;
    blocker.rollback().await.unwrap();
    let (approved, updated) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(approval, metadata)
    })
    .await
    .expect("both operations must finish");
    approved.unwrap().expect("approval must succeed");
    updated
        .unwrap()
        .expect("metadata must succeed even after approval");
    let status: String = sqlx::query_scalar("SELECT status::text FROM claims WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "approved");
    let saved: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM claim_metadata WHERE claim_id=$1 AND owner_user_id=2",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved, json!({"approval":"done", "note":"test"}));
    assert_eq!(get_amount(&pool, MONEY_USER1, 1).await, 199_600);
    assert_eq!(get_amount(&pool, MONEY_USER2, 1).await, 900);
    let payments: i64 = sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(payments, 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn metadata_update_serializes_with_single_approval(pool: PgPool) {
    competing(pool, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn metadata_update_serializes_with_bulk_approval(pool: PgPool) {
    competing(pool, true).await;
}
