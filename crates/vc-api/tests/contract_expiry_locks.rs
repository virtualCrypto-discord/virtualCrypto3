//! A blocked refund must roll back and let independent expiries make progress.

mod support;

use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use support::*;
use time::OffsetDateTime;

async fn expiry_with_lock(pool: PgPool, lock: &'static str) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "expiry locks").await;
    let mut contracts = Vec::new();
    for (account, discord_id) in [(1, MONEY_USER1), (2, MONEY_USER2)] {
        let now = OffsetDateTime::now_utc();
        let id = vc_core::contract::create(
            &pool,
            application,
            "n",
            &[vc_core::contract::NewParty {
                discord_id,
                amount: 100,
            }],
            None,
            Some(3600),
            now,
        )
        .await
        .unwrap();
        vc_core::contract::approve(&pool, id, account, || now)
            .await
            .unwrap();
        contracts.push(id);
    }
    sqlx::query("UPDATE contracts SET expires_at = now() - interval '2 minutes' WHERE id = $1")
        .bind(contracts[0])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE contracts SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(contracts[1])
        .execute(&pool)
        .await
        .unwrap();
    let before = vc_core::contract::find(&pool, contracts[0])
        .await
        .unwrap()
        .unwrap();

    let mut blocker = pool.begin().await.unwrap();
    sqlx::query(lock)
        .bind(contracts[0])
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let recorded = Arc::new(Recorded::default());
    let state = state_with_notifier(pool.clone(), fake(), recorded.clone());
    tokio::time::timeout(
        Duration::from_secs(5),
        vc_api::scheduler::settle_expired(&state),
    )
    .await
    .expect("the tick must finish while the first refund is blocked");

    assert_eq!(
        vc_core::contract::find(&pool, contracts[0])
            .await
            .unwrap()
            .unwrap(),
        before
    );
    let escrow: i64 = sqlx::query_scalar(
        "SELECT a.amount FROM assets a JOIN users u ON u.id=a.user_id WHERE u.contract_id=$1",
    )
    .bind(contracts[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        escrow, 100,
        "a failed refund must roll back its escrow debit"
    );
    assert_eq!(get_amount(&pool, MONEY_USER1, money.currency).await, 199400);
    assert_eq!(get_amount(&pool, MONEY_USER2, money.currency).await, 1000);
    assert_eq!(
        vc_core::contract::find(&pool, contracts[1])
            .await
            .unwrap()
            .unwrap()
            .status,
        "expired"
    );
    assert_eq!(recorded.contract_decisions(), [(application, contracts[1])]);

    blocker.rollback().await.unwrap();
    // The next tick retries the busy contract, and later ticks do not refund twice.
    vc_api::scheduler::settle_expired(&state).await;
    vc_api::scheduler::settle_expired(&state).await;
    let settled = vc_core::contract::find(&pool, contracts[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.status, "expired");
    assert_eq!(settled.remaining, 0);
    assert_eq!(get_amount(&pool, MONEY_USER1, money.currency).await, 199500);
    assert_eq!(get_amount(&pool, MONEY_USER2, money.currency).await, 1000);
    assert_eq!(
        recorded.contract_decisions(),
        [(application, contracts[1]), (application, contracts[0])]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn locked_contract_is_skipped_and_retried(pool: PgPool) {
    expiry_with_lock(pool, "SELECT id FROM contracts WHERE id=$1 FOR UPDATE").await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn locked_refund_recipient_is_skipped_and_retried(pool: PgPool) {
    expiry_with_lock(pool, "SELECT u.id FROM users u JOIN contract_parties p ON p.discord_id=u.discord_id WHERE p.contract_id=$1 FOR NO KEY UPDATE OF u").await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn locked_refund_asset_rolls_back_the_escrow_debit(pool: PgPool) {
    expiry_with_lock(pool, "SELECT a.id FROM assets a JOIN users u ON u.id=a.user_id JOIN contract_parties p ON p.discord_id=u.discord_id JOIN contracts c ON c.id=p.contract_id AND c.currency_id=a.currency_id WHERE c.id=$1 FOR UPDATE OF a").await;
}
