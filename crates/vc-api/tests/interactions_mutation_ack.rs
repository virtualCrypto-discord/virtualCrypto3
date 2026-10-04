mod support;

use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};
use sqlx::PgPool;
use support::*;
use vc_api::claim_list::{ListOptions, Page, Position};
use vc_api::custom_id::ui::{button, contract};

#[derive(Clone, Copy, Debug)]
enum Operation {
    Make,
    Approve,
    Single,
    List,
    ContractApprove,
    ContractRefuse,
    ContractWithdraw,
}

const OPERATIONS: [Operation; 7] = [
    Operation::Make,
    Operation::Approve,
    Operation::Single,
    Operation::List,
    Operation::ContractApprove,
    Operation::ContractRefuse,
    Operation::ContractWithdraw,
];

struct Prepared {
    operation: Operation,
    target: i64,
    payload: Value,
    balance: i64,
    claims: i64,
    histories: i64,
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    let query = match table {
        "claims" => "SELECT count(*) FROM claims",
        "currency_payment_histories" => "SELECT count(*) FROM currency_payment_histories",
        _ => unreachable!(),
    };
    sqlx::query_scalar(query).fetch_one(pool).await.unwrap()
}

async fn ledger(pool: &PgPool) -> Vec<Value> {
    let mut rows = Vec::new();
    for query in [
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id), '[]'::jsonb) FROM assets t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id), '[]'::jsonb) FROM claims t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id), '[]'::jsonb) FROM contracts t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id), '[]'::jsonb) FROM contract_parties t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id), '[]'::jsonb) FROM currency_payment_histories t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id), '[]'::jsonb) FROM users t",
    ] {
        rows.push(sqlx::query_scalar(query).fetch_one(pool).await.unwrap());
    }
    rows
}

async fn prepare(pool: &PgPool, money: &Money, operation: Operation, id: usize) -> Prepared {
    let (target, payload) = match operation {
        Operation::Make => (
            0,
            execute_from_guild(
                json!({"name":"claim", "options":[{
                    "name":"make", "options":[
                        {"name":"user", "value":money.user2.to_string()},
                        {"name":"unit", "value":money.unit}, {"name":"amount", "value":100}
                    ]
                }]}),
                money.user1,
            ),
        ),
        Operation::Approve | Operation::Single | Operation::List => {
            let id = vc_core::claim::create(pool, 1, money.user2, &money.unit, 100, None)
                .await
                .unwrap();
            let payload = if matches!(operation, Operation::Approve) {
                execute_from_guild(
                    json!({"name":"claim", "options":[{
                        "name":"approve", "options":[{"name":"id", "value":id.to_string()}]
                    }]}),
                    money.user2,
                )
            } else {
                let data = if matches!(operation, Operation::Single) {
                    let mut data = button::claim_action_single(button::Action::Approve).to_vec();
                    data.extend_from_slice(&id.to_be_bytes());
                    data
                } else {
                    let mut data = button::claim_action(button::Action::Approve).to_vec();
                    data.extend_from_slice(
                        &ListOptions {
                            pending: true,
                            approved: true,
                            denied: true,
                            canceled: true,
                            position: Position::All,
                            page: Page::Number(1),
                            related_user: None,
                        }
                        .encode(),
                    );
                    data.extend_from_slice(&vc_api::claim_list::encode_claim_ids(&[id]));
                    data
                };
                let custom_id = vc_api::custom_id::encode(0, &data);
                button_from_guild(json!({"custom_id":custom_id}), money.user2)
            };
            (id, payload)
        }
        Operation::ContractApprove | Operation::ContractRefuse | Operation::ContractWithdraw => {
            let application = insert_application(pool, money.user1, "acknowledgement test").await;
            let mut parties = vec![vc_core::contract::NewParty {
                discord_id: money.user2,
                amount: 100,
            }];
            if matches!(operation, Operation::ContractRefuse) {
                parties.push(vc_core::contract::NewParty {
                    discord_id: money.user1,
                    amount: 100,
                });
            }
            let id = vc_core::contract::create(
                pool,
                application,
                &money.unit,
                &parties,
                None,
                None,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
            let action = match operation {
                Operation::ContractRefuse => {
                    vc_core::contract::approve(pool, id, 1, time::OffsetDateTime::now_utc)
                        .await
                        .unwrap();
                    contract::Action::Refuse
                }
                Operation::ContractWithdraw => {
                    vc_core::contract::approve(pool, id, 2, time::OffsetDateTime::now_utc)
                        .await
                        .unwrap();
                    contract::Action::Withdraw
                }
                _ => contract::Action::Approve,
            };
            (
                id,
                button_from_guild(
                    json!({"custom_id":contract::custom_id(action, id)}),
                    money.user2,
                ),
            )
        }
    };
    let mut payload = callback_fields(payload);
    payload["id"] = json!(format!("90000000000001{id:04}"));
    Prepared {
        operation,
        target,
        payload,
        balance: get_amount(pool, money.user2, money.currency).await,
        claims: count(pool, "claims").await,
        histories: count(pool, "currency_payment_histories").await,
    }
}

fn acknowledged(api: &FakeDiscord, prepared: &Prepared) {
    if matches!(prepared.operation, Operation::Make | Operation::Approve) {
        assert_eq!(
            api.callbacks(),
            vec![json!({"type":4,"data":{
                "flags":64,"content":"処理中…","allowed_mentions":{"parse":[]}
            }})]
        );
    } else {
        assert_eq!(api.callbacks(), vec![json!({"type":6})]);
    }
}

async fn finished(api: &FakeDiscord, operation: Operation) {
    tokio::time::timeout(Duration::from_secs(5), async {
        if matches!(operation, Operation::List) {
            api.followup_finished().await;
        } else {
            api.response_edit_finished().await;
        }
    })
    .await
    .expect("operation finished its response attempt");
}

async fn applied(pool: &PgPool, money: &Money, prepared: &Prepared) {
    let delta = match prepared.operation {
        Operation::Make | Operation::ContractRefuse => 0,
        Operation::ContractWithdraw => 100,
        _ => -100,
    };
    assert_eq!(
        get_amount(pool, money.user2, money.currency).await,
        prepared.balance + delta
    );
    assert_eq!(
        count(pool, "claims").await,
        prepared.claims + i64::from(matches!(prepared.operation, Operation::Make))
    );
    assert_eq!(
        count(pool, "currency_payment_histories").await,
        prepared.histories + i64::from(!matches!(prepared.operation, Operation::Make))
    );
    match prepared.operation {
        Operation::Make => {}
        Operation::Approve | Operation::Single | Operation::List => {
            let claim = vc_core::claim::view(pool, 2, prepared.target)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(claim.status.as_deref(), Some("approved"));
        }
        _ => {
            let contract = vc_core::contract::find(pool, prepared.target)
                .await
                .unwrap()
                .unwrap();
            if matches!(prepared.operation, Operation::ContractApprove) {
                assert_eq!(contract.status, "active");
                assert_eq!(contract.remaining, 100);
            } else {
                assert_eq!(contract.status, "canceled");
                assert_eq!(contract.remaining, 0);
            }
        }
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn long_database_waits_are_acknowledged_and_replays_do_not_repeat_mutations(pool: PgPool) {
    let money = setup_money(&pool).await;
    let runtime = vc_core::db::pool_options(4)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    for (index, operation) in OPERATIONS.into_iter().enumerate() {
        let prepared = prepare(&pool, &money, operation, index).await;
        let before = ledger(&pool).await;
        let api = fake();
        let app = vc_api::router(state(runtime.clone(), api.clone()));
        let mut gate = pool.begin().await.unwrap();
        match operation {
            Operation::Make => {
                sqlx::query("SELECT id FROM currencies WHERE id = $1 FOR UPDATE")
                    .bind(money.currency)
                    .fetch_one(&mut *gate)
                    .await
                    .unwrap();
            }
            Operation::ContractRefuse | Operation::ContractWithdraw => {
                sqlx::query("SELECT id FROM contracts WHERE id = $1 FOR UPDATE")
                    .bind(prepared.target)
                    .fetch_one(&mut *gate)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query(
                    "SELECT amount FROM assets WHERE user_id=2 AND currency_id=$1 FOR UPDATE",
                )
                .bind(money.currency)
                .fetch_one(&mut *gate)
                .await
                .unwrap();
            }
        }
        let response = tokio::time::timeout(
            Duration::from_millis(2500),
            interaction(app.clone(), prepared.payload.clone()),
        )
        .await
        .expect("initial acknowledgement precedes the DB wait");
        assert_eq!(response.status, 202, "{operation:?}: {}", response.body);
        acknowledged(&api, &prepared);
        tokio::time::sleep(Duration::from_millis(3200)).await;
        assert!(api.response_edits().is_empty());
        assert!(api.webhooks().is_empty());
        assert_eq!(ledger(&pool).await, before);
        assert_eq!(
            interaction(app.clone(), prepared.payload.clone())
                .await
                .status,
            202
        );
        gate.rollback().await.unwrap();
        finished(&api, operation).await;
        applied(&pool, &money, &prepared).await;
        let after = ledger(&pool).await;
        assert_eq!(interaction(app, prepared.payload.clone()).await.status, 202);
        assert_eq!(ledger(&pool).await, after);
        acknowledged(&api, &prepared);
        assert_eq!(api.response_edits().len(), 1);
        assert_eq!(api.response_edits()[0]["flags"], 32768);
        if matches!(operation, Operation::List) {
            assert_eq!(api.webhooks().len(), 1);
            assert_eq!(api.webhooks()[0]["flags"], 32832);
        } else {
            assert!(api.webhooks().is_empty());
        }
    }
    runtime.close().await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn rejected_or_timed_out_acknowledgements_never_mutate(pool: PgPool) {
    let money = setup_money(&pool).await;
    for (index, operation) in OPERATIONS.into_iter().enumerate() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        for (failure, api) in [
            FakeDiscord::with_callback_error(),
            FakeDiscord::with_callback_gate(gate.clone()),
        ]
        .into_iter()
        .enumerate()
        {
            let prepared = prepare(&pool, &money, operation, index * 2 + failure).await;
            let before = ledger(&pool).await;
            let app = vc_api::router(state(pool.clone(), api.clone()));
            let response = tokio::time::timeout(
                Duration::from_secs(3),
                interaction(app.clone(), prepared.payload.clone()),
            )
            .await
            .unwrap();
            assert_eq!(response.status, 500, "{operation:?}");
            if failure == 1 {
                gate.add_permits(1);
            }
            assert_eq!(interaction(app, prepared.payload).await.status, 500);
            assert_eq!(ledger(&pool).await, before, "{operation:?}");
            assert!(api.callbacks().is_empty());
            assert!(api.response_edits().is_empty());
            assert!(api.webhooks().is_empty());
        }
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn response_edit_failures_do_not_repeat_mutations(pool: PgPool) {
    let money = setup_money(&pool).await;
    for (index, operation) in OPERATIONS.into_iter().enumerate() {
        let prepared = prepare(&pool, &money, operation, index).await;
        let api = FakeDiscord::with_response_edit_error();
        let app = vc_api::router(state(pool.clone(), api.clone()));
        assert_eq!(
            interaction(app.clone(), prepared.payload.clone())
                .await
                .status,
            202
        );
        finished(&api, operation).await;
        applied(&pool, &money, &prepared).await;
        let after = ledger(&pool).await;
        assert_eq!(interaction(app, prepared.payload.clone()).await.status, 202);
        assert_eq!(ledger(&pool).await, after);
        acknowledged(&api, &prepared);
        assert!(api.response_edits().is_empty());
        if matches!(operation, Operation::List) {
            assert!(
                api.webhooks()[0]
                    .to_string()
                    .contains("承諾して支払いました")
            );
        }
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn database_errors_roll_back_and_report_a_private_result(pool: PgPool) {
    let money = setup_money(&pool).await;
    for (index, operation) in OPERATIONS[..5].iter().copied().enumerate() {
        let prepared = prepare(&pool, &money, operation, index).await;
        let (add, drop) = if matches!(operation, Operation::Make) {
            (
                "ALTER TABLE claims ADD CONSTRAINT injected_failure CHECK (amount <> 100) NOT VALID",
                "ALTER TABLE claims DROP CONSTRAINT injected_failure",
            )
        } else {
            (
                "ALTER TABLE currency_payment_histories ADD CONSTRAINT injected_failure CHECK (amount <> 100) NOT VALID",
                "ALTER TABLE currency_payment_histories DROP CONSTRAINT injected_failure",
            )
        };
        sqlx::query(add).execute(&pool).await.unwrap();
        let before = ledger(&pool).await;
        let api = fake();
        let app = vc_api::router(state(pool.clone(), api.clone()));
        assert_eq!(interaction(app, prepared.payload.clone()).await.status, 202);
        if matches!(operation, Operation::Single) {
            tokio::time::timeout(Duration::from_secs(5), api.followup_finished())
                .await
                .unwrap();
        } else {
            finished(&api, operation).await;
        }
        acknowledged(&api, &prepared);
        assert_eq!(ledger(&pool).await, before, "{operation:?}");
        let message = if matches!(operation, Operation::List | Operation::Single) {
            // A single-claim failure is a private follow-up, leaving its screen in place.
            api.webhooks().pop().unwrap()
        } else {
            api.response_edits().pop().unwrap()
        };
        assert!(
            message
                .to_string()
                .contains("処理できたか確認できませんでした"),
            "{message}"
        );
        sqlx::query(drop).execute(&pool).await.unwrap();
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_claim_followup_preserves_the_known_result_on_the_private_screen(pool: PgPool) {
    let money = setup_money(&pool).await;
    let prepared = prepare(&pool, &money, Operation::List, 0).await;
    let api = FakeDiscord::with_followup(true, None);
    let app = vc_api::router(state(pool.clone(), api.clone()));
    assert_eq!(
        interaction(app.clone(), prepared.payload.clone())
            .await
            .status,
        202
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while api.response_edits().len() < 2 {
            api.response_edit_finished().await;
        }
    })
    .await
    .unwrap();
    let edits = api.response_edits();
    assert_eq!(edits[1]["flags"], 32768);
    assert!(edits[1].to_string().contains("承諾して支払いました"));
    assert!(api.webhooks().is_empty());
    applied(&pool, &money, &prepared).await;
    let after = ledger(&pool).await;
    assert_eq!(interaction(app, prepared.payload).await.status, 202);
    assert_eq!(ledger(&pool).await, after);
}
