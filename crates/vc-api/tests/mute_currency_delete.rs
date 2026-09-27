//! Currency mutes must serialize their lookup and insertion with deletion.

mod support;

use sqlx::PgPool;
use std::time::Duration;
use time::OffsetDateTime;
use vc_core::mute::{self, MuteError};

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
    let money = support::setup_money(&pool).await;
    let now = OffsetDateTime::now_utc();
    assert!(
        mute::mute_currency(&pool, 1, &money.unit2, now)
            .await
            .unwrap()
    );
    let mut blocker = pool.begin().await.unwrap();
    let mute = || {
        let pool = pool.clone();
        tokio::spawn(async move { mute::mute_currency(&pool, 1, "n", now).await })
    };
    let delete = || {
        let pool = pool.clone();
        tokio::spawn(async move {
            vc_core::currency::delete(&pool, support::MONEY_GUILD, "delete n").await
        })
    };
    let (muting, deletion) = if deletion_first {
        sqlx::query("SELECT id FROM assets WHERE currency_id=$1 FOR UPDATE")
            .bind(money.currency)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let deletion = delete();
        wait_for_lock(&pool, &["DELETE FROM assets%"]).await;
        let muting = mute();
        wait_for_lock(
            &pool,
            &[
                "SELECT id FROM currencies%FOR KEY SHARE%",
                "INSERT INTO mutes%",
            ],
        )
        .await;
        (muting, deletion)
    } else {
        sqlx::query("SELECT id FROM users WHERE id=1 FOR UPDATE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let muting = mute();
        wait_for_lock(&pool, &["INSERT INTO mutes%"]).await;
        let deletion = delete();
        wait_for_lock(&pool, &["SELECT id, unit, inserted_at FROM currencies%"]).await;
        (muting, deletion)
    };
    blocker.rollback().await.unwrap();
    let (muted, deleted) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(muting, deletion)
    })
    .await
    .expect("muting and deletion must finish");
    let muted = muted.unwrap();
    if deletion_first {
        assert!(matches!(muted, Err(MuteError::NoSuchCurrency)), "{muted:?}");
    } else {
        assert!(muted.expect("the mute must commit before deletion"));
    }
    assert_eq!(
        deleted.unwrap().unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    let currencies: Vec<i64> =
        sqlx::query_scalar("SELECT currency_id FROM mutes WHERE user_id=1 ORDER BY currency_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        currencies,
        vec![money.currency2],
        "only the deleted currency's mute is removed"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deletion_before_muting_reports_missing_currency(pool: PgPool) {
    competing(pool, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deletion_after_muting_removes_the_committed_mute(pool: PgPool) {
    competing(pool, false).await;
}
