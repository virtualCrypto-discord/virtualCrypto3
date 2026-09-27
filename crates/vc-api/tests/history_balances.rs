//! Balance snapshots are part of the money transaction, not reconstructed from today's assets.
mod support;

use serde_json::json;
use sqlx::PgPool;
use support::*;
use vc_core::history::Movement;

const OTHER: i64 = 100_000_000_000_000_003;

async fn snapshots(pool: &PgPool) -> Vec<(Option<i64>, Option<i64>)> {
    sqlx::query_as("SELECT sender_balance_after, receiver_balance_after FROM currency_payment_histories ORDER BY id")
        .fetch_all(pool).await.unwrap()
}

async fn balance(pool: &PgPool, user: i32, unit: &str) -> i64 {
    sqlx::query_scalar("SELECT COALESCE((SELECT a.amount FROM assets a JOIN currencies c ON c.id = a.currency_id WHERE a.user_id = $1 AND c.unit = $2), 0)")
        .bind(i64::from(user)).bind(unit).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn transfers_preserve_both_snapshots_including_zero_and_self_transfer(pool: PgPool) {
    setup_money(&pool).await;
    insert_user(&pool, 9, OTHER).await;
    vc_core::payment::pay_from_discord(&pool, MONEY_USER1, MONEY_USER2, "n", 250)
        .await
        .unwrap();
    vc_core::payment::pay_from_discord(&pool, MONEY_USER2, MONEY_USER2, "n", 50)
        .await
        .unwrap();
    vc_core::payment::pay_from_discord(&pool, MONEY_USER2, OTHER, "n", 1250)
        .await
        .unwrap();
    assert_eq!(balance(&pool, 2, "n").await, 0);
    assert_eq!(
        snapshots(&pool).await,
        vec![
            (Some(199250), Some(1250)),
            (Some(1250), Some(1250)),
            (Some(0), Some(1250)),
        ]
    );
    let page = vc_core::history::payments(&pool, 2, None, None, 1, 5)
        .await
        .unwrap();
    let own: Vec<_> = page
        .rows
        .iter()
        .map(|row| match row {
            Movement::Payment(payment) => payment.balance_after,
            _ => panic!("no issues in this fixture"),
        })
        .collect();
    assert_eq!(own, [Some(0), Some(1250), Some(1250)]);
    // A subsequent credit cannot rewrite the older zero or incoming snapshots.
    vc_core::issue::issue(&pool, MONEY_GUILD, MONEY_USER2, Some(3))
        .await
        .unwrap();
    assert_eq!(snapshots(&pool).await[2].0, Some(0));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bulk_snapshots_follow_input_order_across_repeated_receivers_self_and_currencies(
    pool: PgPool,
) {
    setup_money(&pool).await;
    insert_user(&pool, 9, OTHER).await;
    let mut tx = pool.begin().await.unwrap();
    vc_core::transfer::transfer_bulk(
        &mut tx,
        2,
        &[
            ("n".into(), 1, 100),
            ("n".into(), 2, 50),
            ("w".into(), 1, 400),
            ("n".into(), 1, 250),
            ("n".into(), 9, 600),
        ],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        snapshots(&pool).await,
        vec![
            (Some(900), Some(199600)),
            (Some(900), Some(900)),
            (Some(199600), Some(400)),
            (Some(650), Some(199850)),
            (Some(50), Some(600)),
        ]
    );
    assert_eq!(balance(&pool, 2, "n").await, 50);
    assert_eq!(balance(&pool, 2, "w").await, 199600);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issues_store_recipient_and_pool_balances_but_only_expose_the_authorized_side(
    pool: PgPool,
) {
    setup_money(&pool).await;
    vc_core::issue::issue(&pool, MONEY_GUILD, MONEY_USER2, Some(300))
        .await
        .unwrap();
    vc_core::issue::issue(&pool, MONEY_GUILD, MONEY_USER2, Some(200))
        .await
        .unwrap();
    let stored: Vec<(Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT receiver_balance_after, pool_balance_after FROM currency_given_histories ORDER BY id")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(stored, [(Some(1300), Some(200)), (Some(1500), Some(0))]);
    let token = mint(&pool, 2, &[]).await;
    let application = insert_application(&pool, MONEY_USER1, "history reader").await;
    insert_grant(&pool, application, MONEY_GUILD, &["vc.issue"]).await;
    let guild_token = mint_guild_token(&pool, application, MONEY_GUILD).await;
    let app = vc_api::router(state(pool, fake()));
    let mine = get(app.clone(), "/api/v2/users/@me/transactions", Some(&token)).await;
    assert_eq!(mine.status, 200);
    assert_eq!(mine.body[0]["balance_after"], "1500");
    assert_eq!(mine.body[1]["balance_after"], "1300");
    assert!(mine.body[0].get("pool_balance_after").is_none());
    let issued = get(
        app.clone(),
        "/api/v2/currencies/1/issuances",
        Some(&guild_token),
    )
    .await;
    assert_eq!(issued.status, 200);
    assert_eq!(issued.body[0]["pool_balance_after"], "0");
    assert_eq!(issued.body[1]["pool_balance_after"], "200");
    assert!(issued.body[0].get("balance_after").is_none());
    assert!(issued.body[0].get("receiver_balance_after").is_none());
    let screen = interaction(
        app,
        from_guild(
            json!({"name":"history", "options":[{"name":"issue","type":1}]}),
            MONEY_USER1,
            MONEY_GUILD,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;
    let shown = screen.body.to_string();
    assert!(shown.contains("発行枠残高: **0** `n`"), "{shown}");
    assert!(!shown.contains("1,500"), "{shown}");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn contracts_record_locks_each_received_slice_and_refunds(pool: PgPool) {
    setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "balance history").await;
    let now = time::OffsetDateTime::now_utc();
    let id = vc_core::contract::create(
        &pool,
        application,
        "n",
        &[
            vc_core::contract::NewParty {
                discord_id: MONEY_USER1,
                amount: 100,
            },
            vc_core::contract::NewParty {
                discord_id: MONEY_USER2,
                amount: 60,
            },
        ],
        None,
        None,
        now,
    )
    .await
    .unwrap();
    vc_core::contract::approve(&pool, id, 1, || now)
        .await
        .unwrap();
    vc_core::contract::approve(&pool, id, 2, || now)
        .await
        .unwrap();
    // The first party's 100 comes back; the second party contributes another 20.
    vc_core::contract::pay(&pool, id, application, MONEY_USER1, None, 120, || now)
        .await
        .unwrap();
    vc_core::contract::withdraw(&pool, id, 2, || now)
        .await
        .unwrap();
    assert_eq!(
        snapshots(&pool).await,
        vec![
            (Some(199400), None),
            (Some(940), None),
            (None, Some(199500)),
            (None, Some(199520)),
            (None, Some(980)),
        ]
    );
    let page = vc_core::history::payments(&pool, 1, None, None, 1, 5)
        .await
        .unwrap();
    let entries: Vec<_> = page
        .rows
        .iter()
        .map(|row| match row {
            Movement::Payment(p) => (p.event, p.balance_after),
            _ => panic!("payment expected"),
        })
        .collect();
    assert_eq!(
        entries,
        [
            (Some("charge"), Some(199520)),
            (Some("return"), Some(199500)),
            (Some("lock"), Some(199400))
        ]
    );
    assert_eq!(balance(&pool, 1, "n").await, 199520);
    assert_eq!(balance(&pool, 2, "n").await, 980);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn claim_approval_uses_the_same_atomic_snapshots(pool: PgPool) {
    setup_money(&pool).await;
    let claim = vc_core::claim::create(&pool, 1, MONEY_USER2, "n", 100, None)
        .await
        .unwrap();
    vc_core::claim::transition(
        &pool,
        &vc_core::notification::NoopNotifier,
        2,
        claim,
        vc_core::claim::Transition::Approved,
        None,
    )
    .await
    .unwrap();
    assert_eq!(snapshots(&pool).await, [(Some(900), Some(199600))]);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn rolling_back_a_transfer_rolls_back_its_snapshots(pool: PgPool) {
    setup_money(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    vc_core::transfer::transfer(&mut tx, 2, 1, 100, "n")
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(snapshots(&pool).await.is_empty());
    assert_eq!(balance(&pool, 2, "n").await, 1000);
    assert_eq!(balance(&pool, 1, "n").await, 199500);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn concurrent_issue_and_transfer_capture_serialized_balances(pool: PgPool) {
    setup_money(&pool).await;
    let (sent, issued) = tokio::join!(
        vc_core::payment::pay_from_discord(&pool, MONEY_USER1, MONEY_USER2, "n", 7),
        vc_core::issue::issue(&pool, MONEY_GUILD, MONEY_USER2, Some(5)),
    );
    sent.unwrap();
    issued.unwrap();
    let paid = snapshots(&pool).await[0].1.unwrap();
    let given: i64 =
        sqlx::query_scalar("SELECT receiver_balance_after FROM currency_given_histories")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        (paid == 1007 && given == 1012) || (paid == 1012 && given == 1005),
        "paid={paid}, given={given}"
    );
    assert_eq!(balance(&pool, 2, "n").await, 1012);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn legacy_missing_snapshots_are_not_misrepresented_as_zero_or_current_balances(pool: PgPool) {
    setup_money(&pool).await;
    sqlx::query("INSERT INTO currency_payment_histories (amount,sender_id,receiver_id,currency_id,time,inserted_at,updated_at) VALUES (10,1,2,1,now(),now(),now())")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO currency_given_histories (amount,receiver_id,currency_id,time,inserted_at,updated_at) VALUES (20,2,1,now(),now(),now())")
        .execute(&pool).await.unwrap();
    let token = mint(&pool, 2, &[]).await;
    let app = vc_api::router(state(pool, fake()));
    let api = get(app.clone(), "/api/v2/users/@me/transactions", Some(&token)).await;
    assert_eq!(api.status, 200);
    for row in api.body.as_array().unwrap() {
        assert_eq!(row.get("balance_after"), Some(&serde_json::Value::Null));
        assert!(row.get("sender_balance_after").is_none());
        assert!(row.get("receiver_balance_after").is_none());
    }
    let screen = interaction(
        app,
        execute_from_dm(
            json!({"name":"history", "options":[{"name":"pay","type":1}]}),
            MONEY_USER2,
        ),
    )
    .await;
    assert_eq!(screen.status, 200);
    assert_eq!(
        screen
            .body
            .to_string()
            .matches("取引後残高: 未記録")
            .count(),
        2
    );
}
