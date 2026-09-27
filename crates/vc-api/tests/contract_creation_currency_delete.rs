mod support;

use sqlx::PgPool;
use std::time::Duration;
use support::*;
use vc_core::contract::{self, ContractError, NewParty};

async fn wait_for_lock(pool: &PgPool, pattern: &str) {
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

async fn competing(pool: PgPool, deletion_first: bool) {
    setup_money(&pool).await;
    let app = insert_application(&pool, MONEY_USER1, "currency deletion").await;
    let mut blocker = pool.begin().await.unwrap();
    let create = || {
        let p = pool.clone();
        tokio::spawn(async move {
            contract::create(
                &p,
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
        })
    };
    let delete = || {
        let p = pool.clone();
        tokio::spawn(async move { vc_core::currency::delete(&p, MONEY_GUILD, "delete n").await })
    };
    let (creation, deletion) = if deletion_first {
        // Pause deletion after it has locked the currency.
        sqlx::query("SELECT id FROM assets WHERE currency_id=1 FOR UPDATE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let deletion = delete();
        wait_for_lock(&pool, "DELETE FROM assets%").await;
        let creation = create();
        wait_for_lock(&pool, "SELECT id FROM currencies%FOR KEY SHARE%").await;
        (creation, deletion)
    } else {
        // Pause the contract INSERT at its application foreign-key check.
        sqlx::query("SELECT id FROM applications WHERE id=$1 FOR UPDATE")
            .bind(app)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let creation = create();
        wait_for_lock(&pool, "INSERT INTO contracts%").await;
        let deletion = delete();
        wait_for_lock(&pool, "SELECT id, unit, inserted_at FROM currencies%").await;
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
            matches!(created, Err(ContractError::NotFoundCurrency)),
            "{created:?}"
        );
    } else {
        created.expect("creation must commit before deletion");
    }
    assert_eq!(
        deleted.unwrap().unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    let contracts: i64 = sqlx::query_scalar("SELECT count(*) FROM contracts WHERE currency_id=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(contracts, 0);
    let parties: i64 = sqlx::query_scalar("SELECT count(*) FROM contract_parties")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(parties, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deletion_before_contract_creation(pool: PgPool) {
    competing(pool, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn contract_creation_before_deletion(pool: PgPool) {
    competing(pool, false).await;
}
