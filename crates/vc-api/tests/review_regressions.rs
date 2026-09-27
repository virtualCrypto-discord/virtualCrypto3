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
    payload["application_id"] = json!("123");
    payload["token"] = json!("pay-token");
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
async fn contract_deadlines_use_the_time_after_lock_waits(pool: PgPool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use time::OffsetDateTime;
    use vc_core::contract::{self, ContractError, NewParty};

    #[derive(Clone, Copy, Debug)]
    enum Operation {
        Approve,
        ApproveAfterBalanceLock,
        Pay,
        Withdraw,
    }

    let money = setup_money(&pool).await;
    let application = insert_application(&pool, money.user1, "deadline test").await;
    for operation in [
        Operation::Approve,
        Operation::ApproveAfterBalanceLock,
        Operation::Pay,
        Operation::Withdraw,
    ] {
        let now = OffsetDateTime::now_utc();
        let id = contract::create(
            &pool,
            application,
            "n",
            &[NewParty {
                discord_id: MONEY_USER1,
                amount: 100,
            }],
            None,
            Some(60),
            now,
        )
        .await
        .unwrap();
        if matches!(operation, Operation::Pay | Operation::Withdraw) {
            contract::approve(&pool, id, 1, || now).await.unwrap();
        }
        let before = contract::find(&pool, id).await.unwrap().unwrap();
        let deadline = before.expires_at.unwrap().assume_utc();
        let payer_balance = get_amount(&pool, MONEY_USER1, money.currency).await;
        let receiver_balance = get_amount(&pool, MONEY_USER2, money.currency).await;
        let history_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
                .fetch_one(&pool)
                .await
                .unwrap();

        let mut gate = pool.begin().await.unwrap();
        if matches!(operation, Operation::ApproveAfterBalanceLock) {
            sqlx::query(
                "SELECT amount FROM assets WHERE user_id = 1 AND currency_id = $1 FOR UPDATE",
            )
            .bind(money.currency)
            .execute(&mut *gate)
            .await
            .unwrap();
        } else {
            sqlx::query("SELECT id FROM contracts WHERE id = $1 FOR UPDATE")
                .bind(id)
                .execute(&mut *gate)
                .await
                .unwrap();
        }

        // Move an injected clock while the operation is demonstrably waiting
        // for the lock, without sleeping until a real deadline.
        let expired = Arc::new(AtomicBool::new(false));
        let task_expired = expired.clone();
        let task_pool = pool.clone();
        let pending = tokio::spawn(async move {
            let clock = || {
                if task_expired.load(Ordering::SeqCst) {
                    deadline
                } else {
                    now
                }
            };
            match operation {
                Operation::Approve | Operation::ApproveAfterBalanceLock => {
                    contract::approve(&task_pool, id, 1, clock).await
                }
                Operation::Pay => {
                    contract::pay(&task_pool, id, application, MONEY_USER2, None, 10, clock)
                        .await
                        .map(|_| true)
                }
                Operation::Withdraw => contract::withdraw(&task_pool, id, 1, clock).await,
            }
        });
        wait_for_blocked(&pool, 1).await;
        expired.store(true, Ordering::SeqCst);
        gate.rollback().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap();
        let after = contract::find(&pool, id).await.unwrap().unwrap();
        if matches!(operation, Operation::Withdraw) {
            assert!(result.unwrap());
            assert_eq!(after.remaining, 0);
            assert_eq!(after.parties[0].status, "withdrawn");
            assert_eq!(
                get_amount(&pool, MONEY_USER1, money.currency).await,
                payer_balance + 100
            );
        } else {
            assert!(
                matches!(result, Err(ContractError::Expired)),
                "{operation:?}: {result:?}"
            );
            assert_eq!(after, before);
            assert_eq!(
                get_amount(&pool, MONEY_USER1, money.currency).await,
                payer_balance
            );
            let after_count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(after_count, history_count);
        }
        assert_eq!(
            get_amount(&pool, MONEY_USER2, money.currency).await,
            receiver_balance
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn identical_signed_payment_is_replayed_across_app_instances(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user1, money.currency).await;
    let payload = payment_payload(&money, "900000000000000001");
    let body = serde_json::to_vec(&payload).unwrap();
    let (timestamp, signature) = sign_interaction(&body);
    let discord = fake();
    let mut responses = Vec::new();
    for _ in 0..2 {
        let app = vc_api::router(state(pool.clone(), discord.clone()));
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
            response.headers().get("content-type").cloned(),
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        ));
    }
    assert_eq!(responses[0], responses[1]);
    let result: Value = serde_json::from_slice(&responses[0].1).unwrap();
    assert_eq!(result["type"], 4);
    assert_eq!(result["data"]["flags"], 32768);
    assert!(discord.callbacks().is_empty());
    assert!(discord.webhooks().is_empty());
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
        vc_api::router(state(pool.clone(), discord.clone())),
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
    let discord = fake();
    let owner = tokio::spawn(interaction(
        vc_api::router(state(pool.clone(), discord.clone())),
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
    assert!(discord.callbacks().is_empty());
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

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issuance_and_discord_payments_or_claim_approvals_both_commit(pool: PgPool) {
    use vc_core::claim::{PartialClaim, Transition};
    use vc_core::notification::NoopNotifier;

    let money = setup_money(&pool).await;
    let application = insert_application(&pool, money.user1, "issuer").await;
    let token = insert_grant(&pool, application, money.guild, &["vc.issue"]).await;
    let before_a = get_amount(&pool, money.user1, money.currency).await;
    let before_b = get_amount(&pool, money.user2, money.currency).await;

    // Pause A -> B with A's balance locked, then issue to A. FOR UPDATE on
    // the currency made the payment's foreign-key check wait for the issue,
    // which was already waiting for A's balance.
    sqlx::raw_sql(
        "CREATE FUNCTION issue_payment_gate() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.user_id = 2 AND NEW.amount = 10 THEN
                 PERFORM pg_advisory_xact_lock_shared(778811);
             END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER issue_payment_gate BEFORE INSERT ON assets
         FOR EACH ROW EXECUTE FUNCTION issue_payment_gate();",
    )
    .execute(&pool)
    .await
    .unwrap();

    #[derive(Clone, Copy)]
    enum Payment {
        Discord,
        Bulk,
        Claim,
        BulkClaim,
    }
    for via_api in [false, true] {
        for kind in [
            Payment::Discord,
            Payment::Bulk,
            Payment::Claim,
            Payment::BulkClaim,
        ] {
            let claim_id = if matches!(kind, Payment::Claim | Payment::BulkClaim) {
                Some(
                    vc_core::claim::create(&pool, 2, money.user1, &money.unit, 10, None)
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let mut gate = pool.begin().await.unwrap();
            sqlx::query("SELECT pg_advisory_xact_lock(778811)")
                .execute(&mut *gate)
                .await
                .unwrap();
            let payment_pool = pool.clone();
            let payment = tokio::spawn(async move {
                match kind {
                    Payment::Discord => vc_core::payment::pay_from_discord(
                        &payment_pool,
                        MONEY_USER1,
                        MONEY_USER2,
                        "n",
                        10,
                    )
                    .await
                    .unwrap(),
                    Payment::Bulk => pay(payment_pool, 1, MONEY_USER2, true).await.unwrap(),
                    Payment::Claim => vc_core::claim::transition(
                        &payment_pool,
                        &NoopNotifier,
                        1,
                        claim_id.unwrap(),
                        Transition::Approved,
                        None,
                    )
                    .await
                    .unwrap(),
                    Payment::BulkClaim => {
                        vc_core::claim::update_claims(
                            &payment_pool,
                            &NoopNotifier,
                            1,
                            &[PartialClaim {
                                id: claim_id.unwrap(),
                                status: Some("approved".into()),
                                metadata: None,
                            }],
                        )
                        .await
                        .unwrap();
                    }
                }
            });
            wait_for_blocked(&pool, 1).await;
            let issue_pool = pool.clone();
            let token = token.clone();
            let issuance = tokio::spawn(async move {
                if via_api {
                    assert_eq!(
                        issue_via_api(issue_pool, &token, MONEY_USER1, 10).await,
                        201
                    );
                } else {
                    vc_core::issue::issue(&issue_pool, MONEY_GUILD, MONEY_USER1, Some(10))
                        .await
                        .unwrap();
                }
            });
            wait_for_blocked(&pool, 2).await;
            gate.rollback().await.unwrap();
            let (payment, issuance) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(payment, issuance)
            })
            .await
            .unwrap();
            payment.unwrap();
            issuance.unwrap();
            if let Some(id) = claim_id {
                let status: String =
                    sqlx::query_scalar("SELECT status::text FROM claims WHERE id=$1")
                        .bind(id)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(status, "approved");
            }
        }
    }
    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        before_a
    );
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before_b + 80
    );
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .unwrap()
            .pool_amount,
        Some(420)
    );
    for query in [
        "SELECT count(*) FROM currency_payment_histories",
        "SELECT count(*) FROM currency_given_histories",
    ] {
        let count: i64 = sqlx::query_scalar(query).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 8);
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn concurrent_issuers_cannot_overspend_the_pool(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, money.user1, "issuer").await;
    let token = insert_grant(&pool, application, money.guild, &["vc.issue"]).await;
    let before = get_amount(&pool, money.user1, money.currency).await;
    sqlx::raw_sql(
        "CREATE FUNCTION issuing_gate() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock_shared(778812); RETURN NEW; END $$;
         CREATE TRIGGER issuing_gate BEFORE INSERT ON assets
         FOR EACH ROW EXECUTE FUNCTION issuing_gate();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(778812)")
        .execute(&mut *gate)
        .await
        .unwrap();
    let first_pool = pool.clone();
    let first = tokio::spawn(async move {
        vc_core::issue::issue(&first_pool, MONEY_GUILD, MONEY_USER1, Some(400)).await
    });
    wait_for_blocked(&pool, 1).await;
    let second_pool = pool.clone();
    let second =
        tokio::spawn(async move { issue_via_api(second_pool, &token, MONEY_USER1, 400).await });
    wait_for_blocked(&pool, 2).await;
    gate.rollback().await.unwrap();
    let (first, second) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(first, second)
    })
    .await
    .unwrap();
    assert_eq!(first.unwrap().unwrap().pool_amount, 100);
    assert_eq!(second.unwrap(), 409);
    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        before + 400
    );
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .unwrap()
            .pool_amount,
        Some(100)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM currency_given_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

async fn issue_via_api(pool: PgPool, token: &str, receiver: i64, amount: i64) -> u16 {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v2/currencies/issue")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            json!({
                "receiver_discord_id": receiver.to_string(), "amount": amount.to_string(),
            })
            .to_string(),
        ))
        .unwrap();
    vc_api::router(state(pool, fake()))
        .oneshot(request)
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[derive(Clone, Copy)]
enum Refund {
    Withdraw,
    Refuse,
    Expire,
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn contract_refunds_and_single_or_bulk_payments_both_commit(pool: PgPool) {
    use vc_core::contract::{self, NewParty};

    let money = setup_money(&pool).await;
    let pending_party = money.user2 + 1;
    insert_user(&pool, 3, pending_party).await;
    let application = insert_application(&pool, money.user1, "refund test").await;
    // Hold B -> A after locking B's balance but before writing A's. Before
    // refunds shared the participant locks, a refund could write A and wait
    // for B, leaving both operations waiting for the other's balance lock.
    sqlx::raw_sql(
        "CREATE FUNCTION payment_gate() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.user_id = 1 AND NEW.amount = 10 THEN
                 PERFORM pg_advisory_xact_lock_shared(919293);
             END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER payment_gate BEFORE INSERT ON assets
         FOR EACH ROW EXECUTE FUNCTION payment_gate();",
    )
    .execute(&pool)
    .await
    .unwrap();

    for (ending, expires_in, status) in [
        (Refund::Withdraw, None, "canceled"),
        (Refund::Refuse, None, "canceled"),
        (Refund::Expire, Some(60), "expired"),
    ] {
        for bulk in [false, true] {
            for empty_wallet in [false, true] {
                let before_a = if empty_wallet { 100_i64 } else { 1_000 };
                sqlx::query("UPDATE assets SET amount=$1 WHERE user_id=1 AND currency_id=$2")
                    .bind(before_a)
                    .bind(money.currency)
                    .execute(&pool)
                    .await
                    .unwrap();
                let before_b = get_amount(&pool, money.user2, money.currency).await;
                let now = time::OffsetDateTime::now_utc();
                let id = contract::create(
                    &pool,
                    application,
                    &money.unit,
                    &[
                        NewParty {
                            discord_id: money.user1,
                            amount: 100,
                        },
                        NewParty {
                            discord_id: money.user2,
                            amount: 100,
                        },
                        NewParty {
                            discord_id: pending_party,
                            amount: 100,
                        },
                    ],
                    None,
                    expires_in,
                    now,
                )
                .await
                .unwrap();
                contract::approve(&pool, id, 1, || now).await.unwrap();
                contract::approve(&pool, id, 2, || now).await.unwrap();
                assert_eq!(
                    get_amount(&pool, money.user1, money.currency).await,
                    before_a - 100
                );

                let mut gate = pool.begin().await.unwrap();
                sqlx::query("SELECT pg_advisory_xact_lock(919293)")
                    .execute(&mut *gate)
                    .await
                    .unwrap();
                let payment = tokio::spawn(pay(pool.clone(), 2, money.user1, bulk));
                wait_for_blocked(&pool, 1).await;
                let refund_pool = pool.clone();
                let refund = tokio::spawn(async move {
                    match ending {
                        Refund::Withdraw => contract::withdraw(&refund_pool, id, 1, || now).await,
                        Refund::Refuse => contract::refuse(&refund_pool, id, 3, now).await,
                        Refund::Expire => {
                            contract::settle(&refund_pool, id, now + time::Duration::seconds(60))
                                .await
                        }
                    }
                });
                wait_for_blocked(&pool, 2).await;
                gate.rollback().await.unwrap();
                let (payment, refund) = tokio::time::timeout(Duration::from_secs(5), async {
                    tokio::join!(payment, refund)
                })
                .await
                .unwrap();
                payment.unwrap().unwrap();
                assert!(refund.unwrap().unwrap());

                assert_eq!(
                    get_amount(&pool, money.user1, money.currency).await,
                    before_a + 10
                );
                assert_eq!(
                    get_amount(&pool, money.user2, money.currency).await,
                    before_b - 10
                );
                let contract = contract::find(&pool, id).await.unwrap().unwrap();
                assert_eq!(contract.status, status);
                assert_eq!(contract.remaining, 0);
                assert!(contract.parties.iter().all(|party| party.remaining == 0));
                let records: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM currency_payment_histories WHERE contract_id=$1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(records, 4, "two locks and two refunds");
                let escrow: i64 = sqlx::query_scalar("SELECT COALESCE(sum(a.amount), 0)::bigint FROM assets a JOIN users u ON u.id=a.user_id WHERE u.contract_id=$1")
                    .bind(id).fetch_one(&pool).await.unwrap();
                assert_eq!(escrow, 0);
            }
        }
    }
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
        support::rendered_interaction(
            discord.clone(),
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
    assert_eq!(response.status, 202);
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
    let discord = fake();
    let state = state(pool.clone(), discord.clone());
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
