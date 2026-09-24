mod support;

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::*;
use tower::ServiceExt;

fn payment_payload(money: &Money, id: &str) -> Value {
    let mut payload = execute_from_guild(
        json!({"name":"pay", "options":[
            {"name":"unit", "value":money.unit},
            {"name":"user", "value":money.user2.to_string()},
            {"name":"amount", "value":20}
        ]}),
        money.user1,
    );
    payload["id"] = json!(id);
    payload
}

async fn wait_for_pending(pool: &PgPool, id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let pending: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM discord_interactions WHERE id=$1 AND status IS NULL)",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
            if pending {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("interaction has claimed its receipt");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn identical_signed_payment_is_replayed_across_app_instances(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user1, money.currency).await;
    let payload = payment_payload(&money, "900000000000000001");
    let body = serde_json::to_vec(&payload).unwrap();
    let (timestamp, signature) = sign_interaction(&body);
    let mut responses = Vec::new();
    for _ in 0..2 {
        let app = vc_api::router(state(pool.clone(), fake()));
        let request = Request::builder()
            .method("POST")
            .uri("/api/integrations/discord/interactions")
            .header("content-type", "application/json")
            .header("x-signature-timestamp", &timestamp)
            .header("x-signature-ed25519", &signature)
            .body(Body::from(body.clone()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
        responses.push((
            response.headers()["content-type"].clone(),
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        ));
    }
    assert_eq!(responses[0], responses[1]);
    assert_eq!(
        before - get_amount(&pool, money.user1, money.currency).await,
        20
    );
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(records, 1);

    // An intentional second payment has a new interaction id.
    let response = interaction(
        vc_api::router(state(pool.clone(), fake())),
        payment_payload(&money, "900000000000000002"),
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        before - get_amount(&pool, money.user1, money.currency).await,
        40
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn simultaneous_duplicate_does_not_dispatch_or_cancel_the_owner(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user1, money.currency).await;
    let payload = payment_payload(&money, "900000000000000003");
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=1 FOR NO KEY UPDATE")
        .execute(&mut *gate)
        .await
        .unwrap();
    let owner = tokio::spawn(interaction(
        vc_api::router(state(pool.clone(), fake())),
        payload.clone(),
    ));
    wait_for_pending(&pool, "900000000000000003").await;
    let duplicate = interaction(vc_api::router(state(pool.clone(), fake())), payload.clone()).await;
    assert_eq!(duplicate.status, 409);
    assert_eq!(before, get_amount(&pool, money.user1, money.currency).await);

    // Dropping the HTTP waiter must not cancel a claimed financial operation.
    owner.abort();
    gate.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response =
                interaction(vc_api::router(state(pool.clone(), fake())), payload.clone()).await;
            if response.status == 200 {
                break;
            }
            assert_eq!(response.status, 409);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        before - get_amount(&pool, money.user1, money.currency).await,
        20
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn unfinished_receipt_is_not_reclaimed_after_restart(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user1, money.currency).await;
    sqlx::query("INSERT INTO discord_interactions(id, inserted_at) VALUES ('900000000000000004', now() - interval '1 hour')")
        .execute(&pool).await.unwrap();
    let response = interaction(
        vc_api::router(state(pool.clone(), fake())),
        payment_payload(&money, "900000000000000004"),
    )
    .await;
    assert_eq!(response.status, 409);
    assert_eq!(before, get_amount(&pool, money.user1, money.currency).await);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn opposite_single_and_bulk_payments_both_commit(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before_a = get_amount(&pool, money.user1, money.currency).await;
    let before_b = get_amount(&pool, money.user2, money.currency).await;
    // Pause a transfer at its receiver write while another starts in the
    // opposite direction. Previously both held a sender and deadlocked here.
    sqlx::query("CREATE FUNCTION receiver_gate() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock_shared(818283); RETURN NEW; END $$")
        .execute(&pool).await.unwrap();
    sqlx::query("CREATE TRIGGER receiver_gate BEFORE INSERT ON assets FOR EACH ROW EXECUTE FUNCTION receiver_gate()")
        .execute(&pool).await.unwrap();

    for (left_bulk, right_bulk) in [(false, false), (false, true), (true, true)] {
        let mut gate = pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(818283)")
            .execute(&mut *gate)
            .await
            .unwrap();
        let left = tokio::spawn(pay(pool.clone(), 1, money.user2, left_bulk));
        wait_for_blocked(&pool, 1).await;
        let right = tokio::spawn(pay(pool.clone(), 2, money.user1, right_bulk));
        wait_for_blocked(&pool, 2).await;
        gate.rollback().await.unwrap();
        let (left, right) =
            tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(left, right) })
                .await
                .unwrap();
        left.unwrap().unwrap();
        right.unwrap().unwrap();
    }
    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        before_a
    );
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before_b
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 6);
}

async fn pay(
    pool: PgPool,
    sender: i32,
    receiver: i64,
    bulk: bool,
) -> Result<(), vc_core::payment::PayError> {
    if bulk {
        vc_core::payment::pay_bulk(
            &pool,
            sender,
            &[vc_core::payment::BulkPayment {
                unit: "n".into(),
                receiver_discord_id: receiver,
                amount: 10,
            }],
        )
        .await
    } else {
        vc_core::payment::pay(&pool, sender, receiver, "n", 10).await
    }
}

async fn wait_for_blocked(pool: &PgPool, count: i64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE a.datname=current_database() AND NOT l.granted")
                .fetch_one(pool).await.unwrap();
            if blocked >= count { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("both transfers have reached the contested locks");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_responds_while_discovery_is_blocked_and_later_uses_the_cache(pool: PgPool) {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let discord = FakeDiscord::gated_commands(gate.clone());
    let state = state(pool.clone(), discord.clone());
    let app = vc_api::router(state.clone());
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        interaction(
            app,
            execute_from_guild(
                json!({
                    "name":"create","options":[
                        {"name":"name","value":"ReviewMoney"},
                        {"name":"unit","value":"review"},
                        {"name":"amount","value":1000}
                    ]
                }),
                MONEY_USER1,
            ),
        ),
    )
    .await
    .expect("discovery must not use the three-second interaction budget");
    assert_eq!(response.status, 200);
    assert_eq!(response.body["type"], 4);
    assert!(!response.body.to_string().contains("</info:123>"));
    assert_eq!(discord.command_calls(), 1);
    let created: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM currencies WHERE unit='review')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(created);

    // Concurrent callers share the outstanding lookup instead of starting more.
    let (one, two) = tokio::join!(state.command_ids(), state.command_ids());
    assert!(one.is_empty() && two.is_empty());
    assert_eq!(discord.command_calls(), 1);
    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if state.command_ids().await.get("info") == Some(&123) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(discord.command_calls(), 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn discovery_failure_does_not_permanently_disable_links(pool: PgPool) {
    let discord = FakeDiscord::failing_commands_once();
    let state = state(pool, discord.clone());
    assert!(state.command_ids().await.is_empty());
    assert_eq!(state.command_ids().await.get("info"), Some(&123));
    assert_eq!(discord.command_calls(), 2);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn real_command_options_inherit_the_top_level_id(pool: PgPool) {
    let state = state(
        pool,
        FakeDiscord::with_command_payloads(vec![
            json!({
                "id":"123", "name":"claim", "type":1,
                "options":[
                    {"type":1,"name":"show"},
                    {"type":2,"name":"group","options":[{"type":1,"name":"make"}]}
                ]
            }),
            json!({"name":"unregistered", "options":[{"type":1,"name":"show"}]}),
        ]),
    );
    let ids = state.command_ids().await;
    assert_eq!(ids.get("claim show"), Some(&123));
    assert_eq!(ids.get("claim group make"), Some(&123));
    assert!(!ids.contains_key("unregistered show"));
    assert_eq!(
        vc_api::docs::discord::mentions("`/claim show`", ids),
        "</claim show:123>"
    );
    assert_eq!(
        vc_api::docs::discord::mentions("`/claim group make amount:2`", ids),
        "</claim group make:123> `amount:2`"
    );
    assert_eq!(
        vc_api::docs::discord::mentions("`/claim group unknown`", ids),
        "`/claim group unknown`"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn receipts_and_response_bodies_expire_without_allowing_old_signed_payments(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user1, money.currency).await;
    sqlx::query(
        "INSERT INTO discord_interactions(id, status, body, inserted_at) VALUES
         ('900000000000000010', 200, 'private response', now() - interval '25 hours'),
         ('900000000000000011', NULL, NULL, now() - interval '25 hours'),
         ('900000000000000012', 200, 'recent response', now() - interval '1 hour'),
         ('900000000000000013', NULL, NULL, now() - interval '1 hour')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let state = state(pool.clone(), fake());
    vc_api::scheduler::purge_expired(&state).await;
    vc_api::scheduler::purge_expired(&state).await;
    let kept: Vec<String> = sqlx::query_scalar("SELECT id FROM discord_interactions ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(kept, ["900000000000000012", "900000000000000013"]);

    // Signatures still verify cryptographically, but their time is no longer
    // acceptable. Test both an expired receipt and a request with no receipt.
    for (id, age) in [
        ("900000000000000010", 25 * 60 * 60),
        ("900000000000000011", 25 * 60 * 60),
        ("900000000000000014", 6 * 60),
        ("900000000000000015", -120),
    ] {
        let payload = payment_payload(&money, id);
        let body = serde_json::to_vec(&payload).unwrap();
        let timestamp = (time::OffsetDateTime::now_utc().unix_timestamp() - age).to_string();
        let signature = sign_interaction_at(&body, &timestamp);
        let request = Request::builder()
            .method("POST")
            .uri("/api/integrations/discord/interactions")
            .header("content-type", "application/json")
            .header("x-signature-timestamp", timestamp)
            .header("x-signature-ed25519", signature)
            .body(Body::from(body))
            .unwrap();
        let response = vc_api::router(state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
    }
    assert_eq!(get_amount(&pool, money.user1, money.currency).await, before);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM discord_interactions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "expired requests did not recreate their receipts");
    let fresh = interaction(
        vc_api::router(state),
        payment_payload(&money, "900000000000000016"),
    )
    .await;
    assert_eq!(fresh.status, 200);
    assert_eq!(
        before - get_amount(&pool, money.user1, money.currency).await,
        20
    );
}
