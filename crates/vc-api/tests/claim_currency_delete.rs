mod support;

use sqlx::PgPool;
use std::time::Duration;
use support::*;
use vc_core::claim::{self, PartialClaim, Transition};
use vc_core::notification::NoopNotifier;

async fn wait(pool: &PgPool, patterns: &[&str]) {
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

async fn approve(pool: &PgPool, ids: &[i64], bulk: bool) -> Result<(), String> {
    if bulk {
        let changes: Vec<_> = ids
            .iter()
            .map(|id| PartialClaim {
                id: *id,
                status: Some("approved".into()),
                metadata: None,
            })
            .collect();
        claim::update_claims(pool, &NoopNotifier, 2, &changes)
            .await
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
    } else {
        claim::transition(pool, &NoopNotifier, 2, ids[0], Transition::Approved, None)
            .await
            .map_err(|e| format!("{e:?}"))
    }
}

async fn competing(pool: PgPool, bulk: bool, deletion_first: bool) {
    setup_money(&pool).await;
    let mut ids = vec![
        claim::create(&pool, 1, MONEY_USER2, "n", 100, None)
            .await
            .unwrap(),
    ];
    if bulk {
        // A second currency and repeated first currency exercise batch locking.
        ids.push(
            claim::create(&pool, 1, MONEY_USER2, "w", 100, None)
                .await
                .unwrap(),
        );
        ids.push(
            claim::create(&pool, 1, MONEY_USER2, "n", 100, None)
                .await
                .unwrap(),
        );
        ids.reverse();
    }
    let mut blocker = pool.begin().await.unwrap();
    let (approval, deletion) =
        if deletion_first {
            sqlx::query("SELECT id FROM claims WHERE id=ANY($1) ORDER BY id FOR UPDATE")
                .bind(&ids)
                .execute(&mut *blocker)
                .await
                .unwrap();
            let p = pool.clone();
            let deletion = tokio::spawn(async move {
                vc_core::currency::delete(&p, MONEY_GUILD, "delete n").await
            });
            wait(&pool, &["DELETE FROM claims%"]).await;
            let p = pool.clone();
            let approval = tokio::spawn(async move { approve(&p, &ids, bulk).await });
            wait(&pool, &["SELECT id FROM currencies%"]).await;
            (approval, deletion)
        } else {
            sqlx::query("SELECT id FROM users WHERE id=1 FOR NO KEY UPDATE")
                .execute(&mut *blocker)
                .await
                .unwrap();
            let p = pool.clone();
            let approval = tokio::spawn(async move { approve(&p, &ids, bulk).await });
            wait(&pool, &["SELECT id FROM users WHERE id = ANY%"]).await;
            let p = pool.clone();
            let deletion = tokio::spawn(async move {
                vc_core::currency::delete(&p, MONEY_GUILD, "delete n").await
            });
            // The old implementation reaches DELETE FROM claims and then deadlocks
            // on release; the fixed implementation waits at the currency instead.
            wait(
                &pool,
                &[
                    "SELECT id, unit, inserted_at FROM currencies%",
                    "DELETE FROM claims%",
                ],
            )
            .await;
            (approval, deletion)
        };
    blocker.rollback().await.unwrap();
    let (approved, deleted) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(approval, deletion)
    })
    .await
    .expect("both operations must finish");
    if deletion_first {
        assert_eq!(approved.unwrap(), Err("NotFound".into()));
    } else {
        approved
            .unwrap()
            .expect("approval must commit without deadlocking");
    }
    assert_eq!(
        deleted.unwrap().unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    let assets: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE currency_id=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(assets, 0);
    assert_eq!(
        get_amount(&pool, MONEY_USER2, 2).await,
        if bulk && !deletion_first {
            199_900
        } else {
            200_000
        }
    );
    assert_eq!(
        get_amount(&pool, MONEY_USER1, 2).await,
        if bulk && !deletion_first { 100 } else { 0 }
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn single_approval_before_deletion(pool: PgPool) {
    competing(pool, false, false).await;
}
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bulk_approval_before_deletion(pool: PgPool) {
    competing(pool, true, false).await;
}
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deletion_before_single_approval(pool: PgPool) {
    competing(pool, false, true).await;
}
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deletion_before_bulk_approval(pool: PgPool) {
    competing(pool, true, true).await;
}
