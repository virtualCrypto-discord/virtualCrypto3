mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use support::*;
use vc_api::custom_id::ui::{developer, grant, modal, mute, pat};

#[derive(Clone, Copy, Debug)]
enum Operation {
    Create,
    PatCreate,
    PatRevoke,
    GrantApprove,
    GrantRevoke,
    MuteCurrency,
    MuteUser,
    UnmuteCurrency,
    UnmuteUser,
    UnmuteCurrencyButton,
    UnmuteUserButton,
    Register,
    EditForm,
    EditSelect,
    RotateSecret,
    Connect,
    Delete,
}
const OPERATIONS: [Operation; 17] = [
    Operation::Create,
    Operation::PatCreate,
    Operation::PatRevoke,
    Operation::GrantApprove,
    Operation::GrantRevoke,
    Operation::MuteCurrency,
    Operation::MuteUser,
    Operation::UnmuteCurrency,
    Operation::UnmuteUser,
    Operation::UnmuteCurrencyButton,
    Operation::UnmuteUserButton,
    Operation::Register,
    Operation::EditForm,
    Operation::EditSelect,
    Operation::RotateSecret,
    Operation::Connect,
    Operation::Delete,
];
const BOT: i64 = 200_000_000_000_000_003;

struct Prepared {
    payload: Value,
    bot_description: Option<String>,
}
impl Prepared {
    fn api(&self) -> Arc<FakeDiscord> {
        match &self.bot_description {
            Some(description) => Arc::new(FakeDiscord::with_integrations(
                json!({"name":"TestGuild"}),
                &[(BOT, description)],
            )),
            None => fake(),
        }
    }
}

fn command(money: &Money, name: &str, subcommand: &str, options: Value) -> Value {
    execute_from_dm(
        json!({"name":name,"options":[{
            "name":subcommand,"type":1,"options":options
        }]}),
        money.user1,
    )
}

async fn prepare(pool: &PgPool, money: &Money, operation: Operation, index: usize) -> Prepared {
    use Operation::*;
    let now = time::OffsetDateTime::now_utc();
    let mut bot_description = None;
    let payload = match operation {
        Create => execute_from_guild(
            json!({"name":"create","options":[
                {"name":"amount","value":1000},{"name":"unit","value":"ack"},
                {"name":"name","value":"acknowledged"}
            ]}),
            money.user1,
        ),
        PatCreate => command(
            money,
            "pat",
            "create",
            json!([{"name":"name","value":"created"}]),
        ),
        PatRevoke => {
            vc_auth::issue::personal_token(pool, JWT_SECRET.as_bytes(), 1, "revoked", now)
                .await
                .unwrap();
            let token_id = vc_auth::issue::personal_tokens(pool, 1)
                .await
                .unwrap()
                .into_iter()
                .find(|token| token.name == "revoked")
                .unwrap()
                .token_id;
            button_from_guild(
                json!({"custom_id":pat::revoke(money.user1, 1, token_id)}),
                money.user1,
            )
        }
        GrantApprove | GrantRevoke => {
            let application = insert_application(pool, money.user1, "ack grant").await;
            let custom_id = if matches!(operation, GrantApprove) {
                let asked = vc_core::grant::request_grant(
                    pool,
                    application,
                    vc_core::grant::Target::Guild(money.guild),
                    &["vc.issue".to_owned()],
                    &[],
                    600,
                    now,
                )
                .await
                .unwrap();
                grant::confirm_custom_id(asked.id)
            } else {
                vc_core::grant::allow_in_guild(
                    pool,
                    application,
                    money.guild,
                    &["vc.issue"],
                    &[],
                    now,
                )
                .await
                .unwrap();
                let id = vc_core::grant::grant_for(pool, application, money.guild)
                    .await
                    .unwrap()
                    .unwrap();
                grant::revoke_one_custom_id(id)
            };
            let mut payload = button_from_guild(json!({"custom_id":custom_id}), money.user1);
            payload["guild_id"] = json!(money.guild.to_string());
            payload
        }
        MuteCurrency | MuteUser | UnmuteCurrency | UnmuteUser | UnmuteCurrencyButton
        | UnmuteUserButton => {
            let currency = matches!(
                operation,
                MuteCurrency | UnmuteCurrency | UnmuteCurrencyButton
            );
            let removing = !matches!(operation, MuteCurrency | MuteUser);
            if removing {
                if currency {
                    vc_core::mute::mute_currency(pool, 1, &money.unit, now)
                        .await
                        .unwrap();
                } else {
                    vc_core::mute::mute_user(pool, 1, money.user2, now)
                        .await
                        .unwrap();
                }
            }
            if matches!(operation, UnmuteCurrencyButton | UnmuteUserButton) {
                let target = if currency {
                    vc_core::mute::Target::Currency {
                        id: money.currency,
                        unit: money.unit.clone(),
                        name: money.name.clone(),
                    }
                } else {
                    vc_core::mute::Target::User {
                        discord_id: money.user2,
                    }
                };
                button_from_guild(
                    json!({"custom_id":mute::unmute_custom_id(&target)}),
                    money.user1,
                )
            } else {
                command(
                    money,
                    if removing { "unmute" } else { "mute" },
                    if currency { "currency" } else { "user" },
                    json!([{"name":if currency {"unit"} else {"user"},
                        "value":if currency {money.unit.clone()} else {money.user2.to_string()}}]),
                )
            }
        }
        Register => command(money, "application", "register", json!([])),
        EditForm | EditSelect | RotateSecret | Connect => {
            let application = insert_application(pool, money.user1, "ack application").await;
            let client = client_id_of(pool, application).await;
            if matches!(operation, EditForm) {
                json!({"type":5,"user":{"id":money.user1.to_string()},"data":{
                    "custom_id":developer::custom_id_for_field(developer::Screen::Edit,&client,"client_name"),
                    "components":[{"type":18,"component":{"type":4,"custom_id":"client_name","value":"changed"}}]
                }})
            } else if matches!(operation, RotateSecret) {
                button_from_guild(
                    json!({"custom_id":developer::custom_id_for_field(developer::Screen::RotateSecret,&client,"confirm")}),
                    money.user1,
                )
            } else if matches!(operation, EditSelect) {
                let mut payload = button_from_guild(
                    json!({
                        "custom_id":developer::custom_id_for_field(developer::Screen::Edit,&client,"application_type"),
                        "values":["native"]
                    }),
                    money.user1,
                );
                payload["data"]["component_type"] = json!(3);
                payload
            } else {
                bot_description = Some(format!(
                    "{}/applications/verification?q={client}",
                    links().site_url
                ));
                button_from_guild(
                    json!({
                        "custom_id":developer::custom_id_for_field(developer::Screen::ConfirmConnect,&client,&BOT.to_string())
                    }),
                    money.user1,
                )
            }
        }
        Delete => json!({"type":5,"guild_id":money.guild.to_string(),
            "member":{"user":{"id":money.user1.to_string()},"permissions":DEFAULT_PERMISSIONS},
            "data":{"custom_id":vc_api::custom_id::encode(0,&modal::confirm_currency_delete()),
                "components":[{"type":18,"component":{"custom_id":"confirm","value":format!("delete {}",money.unit)}}]}
        }),
    };
    let mut payload = callback_fields(payload);
    payload["id"] = json!(format!("90000000000002{index:04}"));
    Prepared {
        payload,
        bot_description,
    }
}

async fn snapshot(pool: &PgPool) -> Vec<Value> {
    let mut rows = Vec::new();
    for query in [
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)),'[]'::jsonb) FROM users t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)),'[]'::jsonb) FROM currencies t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)),'[]'::jsonb) FROM assets t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)),'[]'::jsonb) FROM applications t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)),'[]'::jsonb) FROM user_access_tokens t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)),'[]'::jsonb) FROM grant_requests t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)),'[]'::jsonb) FROM grants t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)),'[]'::jsonb) FROM grant_scopes t",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)),'[]'::jsonb) FROM mutes t",
    ] {
        rows.push(sqlx::query_scalar(query).fetch_one(pool).await.unwrap());
    }
    rows
}

fn acknowledged(api: &FakeDiscord, operation: Operation, payload: &Value) {
    let callbacks = api.callbacks();
    assert_eq!(callbacks.len(), 1, "{operation:?}");
    if payload["type"] == 3 {
        assert_eq!(callbacks[0], json!({"type":6}), "{operation:?}");
    } else {
        assert_eq!(
            callbacks[0]["type"],
            if matches!(operation, Operation::EditForm) {
                5
            } else {
                4
            }
        );
        assert_eq!(callbacks[0]["data"]["flags"], 64);
    }
}

async fn finished(api: &FakeDiscord) {
    tokio::time::timeout(Duration::from_secs(5), api.response_edit_finished())
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn management_mutations_acknowledge_before_database_waits_and_replays_do_not_repeat_them(
    pool: PgPool,
) {
    let money = setup_money(&pool).await;
    let runtime = vc_core::db::pool_options(4)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    for (index, operation) in OPERATIONS.into_iter().enumerate() {
        let prepared = prepare(&pool, &money, operation, index).await;
        let api = prepared.api();
        let app = vc_api::router(state(runtime.clone(), api.clone()));
        let before = snapshot(&pool).await;
        let mut gate = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE users, currencies, applications, user_access_tokens, grants, grant_requests, mutes IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *gate).await.unwrap();
        let started = Instant::now();
        let response = tokio::time::timeout(
            Duration::from_millis(2500),
            interaction(app.clone(), prepared.payload.clone()),
        )
        .await
        .unwrap();
        assert_eq!(response.status, 202, "{operation:?}");
        acknowledged(&api, operation, &prepared.payload);
        assert!(api.response_edits().is_empty());
        assert_eq!(
            interaction(app.clone(), prepared.payload.clone())
                .await
                .status,
            202
        );
        tokio::time::sleep(Duration::from_millis(3200).saturating_sub(started.elapsed())).await;
        assert!(api.response_edits().is_empty());
        gate.rollback().await.unwrap();
        finished(&api).await;
        let after = snapshot(&pool).await;
        assert_ne!(after, before, "{operation:?} must apply its mutation");
        assert_eq!(api.response_edits()[0]["flags"], 32768);
        assert!(api.webhooks().is_empty());
        assert_eq!(interaction(app, prepared.payload).await.status, 202);
        assert_eq!(api.callbacks().len(), 1);
        assert_eq!(
            snapshot(&pool).await,
            after,
            "{operation:?} must not run again"
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn rejected_and_timed_out_management_callbacks_leave_domain_data_unchanged(pool: PgPool) {
    let money = setup_money(&pool).await;
    for (index, operation) in OPERATIONS.into_iter().enumerate() {
        let mut prepared = prepare(&pool, &money, operation, index * 2).await;
        let before = snapshot(&pool).await;
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        for (failure, api) in [
            FakeDiscord::with_callback_error(),
            FakeDiscord::with_callback_gate(gate.clone()),
        ]
        .into_iter()
        .enumerate()
        {
            prepared.payload["id"] = json!(format!("90000000000003{:04}", index * 2 + failure));
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
            assert_eq!(interaction(app, prepared.payload.clone()).await.status, 500);
            assert_eq!(snapshot(&pool).await, before, "{operation:?}");
            assert!(api.callbacks().is_empty());
            assert!(api.response_edits().is_empty());
            assert!(api.webhooks().is_empty());
        }
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_management_result_delivery_does_not_repeat_the_mutation(pool: PgPool) {
    let money = setup_money(&pool).await;
    for (index, operation) in [
        Operation::PatCreate,
        Operation::GrantApprove,
        Operation::EditForm,
        Operation::RotateSecret,
        Operation::Delete,
    ]
    .into_iter()
    .enumerate()
    {
        let prepared = prepare(&pool, &money, operation, index).await;
        let before = snapshot(&pool).await;
        let api = FakeDiscord::with_response_edit_error();
        let app = vc_api::router(state(pool.clone(), api.clone()));
        assert_eq!(
            interaction(app.clone(), prepared.payload.clone())
                .await
                .status,
            202
        );
        finished(&api).await;
        let after = snapshot(&pool).await;
        assert_ne!(after, before, "{operation:?}");
        assert_eq!(interaction(app, prepared.payload).await.status, 202);
        assert_eq!(snapshot(&pool).await, after);
        assert_eq!(api.callbacks().len(), 1);
        assert!(api.response_edits().is_empty());
    }
}
