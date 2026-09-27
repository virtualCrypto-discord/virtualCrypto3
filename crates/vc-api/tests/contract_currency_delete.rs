mod support;

use sqlx::PgPool;
use std::time::Duration;
use support::*;
use vc_core::contract::{self, NewParty};

async fn fixture(pool: &PgPool) -> (i64, i64) {
    setup_money(pool).await;
    let app = insert_application(pool, MONEY_USER1, "currency deletion").await;
    let id = contract::create(
        pool,
        app,
        "n",
        &[NewParty {
            discord_id: MONEY_USER1,
            amount: 100,
        }],
        None,
        None,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    contract::approve(pool, id, 1, time::OffsetDateTime::now_utc)
        .await
        .unwrap();
    (app, id)
}

async fn wait_for_lock(pool: &PgPool, pid: i32) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock')",
            ).bind(pid).fetch_one(pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("operation should reach its lock wait");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn charge_finishes_before_concurrent_currency_deletion(pool: PgPool) {
    let (app, id) = fixture(&pool).await;
    // Pause the charge after it has acquired its domain locks, at the escrow
    // debit. This exercises the actual pay_in lock ordering, not a simulation.
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT a.id FROM assets a JOIN users u ON u.id=a.user_id WHERE u.contract_id=$1 FOR UPDATE OF a")
        .bind(id).execute(&mut *blocker).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let charge = tokio::spawn(async move {
        let paid = contract::pay_in(
            &mut tx,
            id,
            app,
            MONEY_USER2,
            None,
            1,
            time::OffsetDateTime::now_utc,
        )
        .await?;
        tx.commit()
            .await
            .map_err(contract::ContractError::Database)?;
        Ok::<_, contract::ContractError>(paid)
    });
    wait_for_lock(&pool, pid).await;
    let p = pool.clone();
    let deletion =
        tokio::spawn(async move { vc_core::currency::delete(&p, MONEY_GUILD, "delete n").await });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND (query LIKE 'SELECT id, unit, inserted_at FROM currencies%' OR query LIKE 'DELETE FROM assets%'))",
            ).fetch_one(&pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    blocker.rollback().await.unwrap();
    let (paid, deleted) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(charge, deletion)
    })
    .await
    .expect("both operations must finish");
    assert_eq!(paid.unwrap().unwrap().remaining, 99);
    assert_eq!(
        deleted.unwrap().unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    let assets: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE currency_id=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(assets, 0);
    assert!(contract::find(&pool, id).await.unwrap().is_none());
    assert_eq!(get_amount(&pool, MONEY_USER2, 2).await, 200_000);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn charge_after_deletion_starts_reports_not_found(pool: PgPool) {
    let (app, id) = fixture(&pool).await;
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM contracts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let p = pool.clone();
    let deletion =
        tokio::spawn(async move { vc_core::currency::delete(&p, MONEY_GUILD, "delete n").await });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'DELETE FROM contracts%')",
            ).fetch_one(&pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let charge = tokio::spawn(async move {
        contract::pay_in(
            &mut tx,
            id,
            app,
            MONEY_USER2,
            None,
            1,
            time::OffsetDateTime::now_utc,
        )
        .await
    });
    wait_for_lock(&pool, pid).await;
    blocker.rollback().await.unwrap();
    let (paid, deleted) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(charge, deletion)
    })
    .await
    .expect("both operations must finish");
    assert!(matches!(
        paid.unwrap(),
        Err(contract::ContractError::NotFound)
    ));
    assert_eq!(
        deleted.unwrap().unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    assert_eq!(get_amount(&pool, MONEY_USER2, 2).await, 200_000);
}
