//! The `issue` command, which the Elixir called `give`.
//!
//! Elixir has no test for this command — `Money.give/1` is only exercised through
//! `setup_money/1` — so these cases are additions rather than a port, and they
//! follow `Command.handle/4` and `Query.Issue.issue/3` directly.

mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::{Duration, Instant};
use support::{
    DEFAULT_PERMISSIONS, FakeDiscord, currency_by_unit, fake, get_amount, interaction, setup_money,
    state,
};

const COLOR_OK: i64 = 0x0038_EA42;

async fn issue(pool: PgPool, payload: Value) -> Value {
    let discord = fake();
    let response = interaction(vc_api::router(state(pool, discord.clone())), payload).await;
    assert_eq!(response.status, 202, "{}", response.body);
    assert_acknowledgement(&discord);
    finished(&discord).await;
    let edits = discord.response_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["flags"], 32768);
    assert_eq!(edits[0].get("content"), Some(&Value::Null));
    assert_eq!(edits[0]["embeds"], json!([]));
    assert!(discord.webhooks().is_empty());
    assert_eq!(discord.response_deletions(), 0);
    edits[0].clone()
}

async fn finished(discord: &FakeDiscord) {
    tokio::time::timeout(Duration::from_secs(5), discord.response_edit_finished())
        .await
        .expect("issuance completed its response attempt");
}

fn assert_acknowledgement(discord: &FakeDiscord) {
    assert_eq!(
        discord.callbacks(),
        vec![json!({
            "type": 4,
            "data": {"flags": 64, "content": "処理中…", "allowed_mentions": {"parse": []}}
        })]
    );
}

fn callback_fields(mut payload: Value) -> Value {
    payload["application_id"] = json!("123");
    payload["token"] = json!("issue-token");
    payload
}

fn issue_data(receiver: i64, amount: Option<Value>) -> Value {
    let mut options = vec![json!({ "name": "user", "value": receiver.to_string() })];
    if let Some(amount) = amount {
        options.push(json!({ "name": "amount", "value": amount }));
    }

    json!({ "name": "issue", "options": options })
}

fn from_guild(
    receiver: i64,
    amount: Option<Value>,
    sender: i64,
    guild_id: i64,
    permissions: &str,
) -> Value {
    callback_fields(support::from_guild(
        issue_data(receiver, amount),
        sender,
        guild_id,
        permissions,
    ))
}

fn assert_error(response: &Value, content: &str) {
    assert_eq!(
        response["components"],
        json!([{
            "type": 17,
            "components": [{ "type": 10, "content": content }],
        }])
    );
    assert_eq!(response["flags"], json!(32768));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issuing_from_the_pool_credits_the_receiver(pool: PgPool) {
    let money = setup_money(&pool).await;
    let receiver_before = get_amount(&pool, money.user2, money.currency).await;

    let response = issue(
        pool.clone(),
        from_guild(
            money.user2,
            Some(json!(100)),
            money.user1,
            money.guild,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_eq!(
        response["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_OK,
            "components": [{
                "type": 10,
                "content": format!(
                    "✅ <@{}>へ**100** `{}`発行されました。\n残りの発行枠: **400** `{}`",
                    money.user2, money.unit, money.unit
                ),
            }],
        }])
    );
    assert_eq!(response["flags"], json!(32768));

    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        receiver_before + 100
    );
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .expect("the currency")
            .pool_amount,
        Some(400)
    );
}

/// Without an amount the `handle/4` clause fills in `:all`, which issues what
/// the pool holds.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issuing_without_an_amount_issues_the_whole_pool(pool: PgPool) {
    let money = setup_money(&pool).await;
    let receiver_before = get_amount(&pool, money.user2, money.currency).await;

    let response = issue(
        pool.clone(),
        from_guild(
            money.user2,
            None,
            money.user1,
            money.guild,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_eq!(
        response["components"][0]["components"][0]["content"],
        json!(format!(
            "✅ <@{}>へ**500** `{}`発行されました。\n残りの発行枠: **0** `{}`",
            money.user2, money.unit, money.unit
        ))
    );

    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        receiver_before + 500
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issuing_more_than_the_pool_is_refused(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = issue(
        pool,
        from_guild(
            money.user2,
            Some(json!(501)),
            money.user1,
            money.guild,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_error(&response, "エラー: 通貨が不足しています。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issuing_needs_the_administrator_bit(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = issue(
        pool,
        from_guild(money.user2, Some(json!(100)), money.user1, money.guild, "0"),
    )
    .await;

    assert_error(&response, "エラー: 実行には管理者権限が必要です。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issuing_in_a_direct_message_is_refused(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = issue(
        pool,
        callback_fields(support::execute_from_dm(
            issue_data(money.user2, Some(json!(100))),
            money.user1,
        )),
    )
    .await;

    assert_error(&response, "エラー: DMでは実行できません。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_currency_lock_longer_than_three_seconds_is_acknowledged_and_issued_once(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let discord = fake();
    let api_pool = vc_core::db::pool_options(4)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let app = vc_api::router(state(api_pool.clone(), discord.clone()));
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM currencies WHERE id = $1 FOR UPDATE")
        .bind(money.currency)
        .fetch_one(&mut *gate)
        .await
        .unwrap();
    let mut payload = from_guild(
        money.user2,
        Some(json!(100)),
        money.user1,
        money.guild,
        DEFAULT_PERMISSIONS,
    );
    payload["id"] = json!("900000000000004001");
    let started = Instant::now();
    let response = tokio::time::timeout(
        Duration::from_millis(2500),
        interaction(app.clone(), payload.clone()),
    )
    .await
    .expect("acknowledgement must not wait for the issuance lock");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(response.status, 202);
    assert_acknowledgement(&discord);
    tokio::time::sleep(Duration::from_millis(3200)).await;
    assert!(discord.response_edits().is_empty());
    assert_eq!(get_amount(&pool, money.user2, money.currency).await, before);
    assert_eq!(interaction(app.clone(), payload.clone()).await.status, 202);
    gate.rollback().await.unwrap();
    finished(&discord).await;
    assert_eq!(interaction(app, payload).await.status, 202);
    assert_acknowledgement(&discord);
    assert_eq!(discord.response_edits().len(), 1);
    assert_eq!(discord.response_edits()[0]["flags"], 32768);
    assert!(discord.webhooks().is_empty());
    assert_eq!(discord.response_deletions(), 0);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 100
    );
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .unwrap()
            .pool_amount,
        Some(400)
    );
    let records: i64 =
        sqlx::query_scalar("SELECT count(*) FROM currency_given_histories WHERE receiver_id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(records, 1);
    api_pool.close().await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_rejected_or_timed_out_acknowledgement_never_issues(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    for (index, discord) in [
        FakeDiscord::with_callback_error(),
        FakeDiscord::with_callback_gate(gate.clone()),
    ]
    .into_iter()
    .enumerate()
    {
        let mut payload = from_guild(
            money.user2,
            Some(json!(100)),
            money.user1,
            money.guild,
            DEFAULT_PERMISSIONS,
        );
        payload["id"] = json!((900_000_000_000_005_000u64 + index as u64).to_string());
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            interaction(
                vc_api::router(state(pool.clone(), discord.clone())),
                payload.clone(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(response.status, 500);
        assert!(discord.callbacks().is_empty());
        assert!(discord.response_edits().is_empty());
        let retry_discord = fake();
        let retry = interaction(
            vc_api::router(state(pool.clone(), retry_discord.clone())),
            payload,
        )
        .await;
        assert_eq!(retry.status, 500);
        assert!(retry_discord.callbacks().is_empty());
    }
    gate.add_permits(1);
    assert_eq!(get_amount(&pool, money.user2, money.currency).await, before);
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .unwrap()
            .pool_amount,
        Some(500)
    );
    let records: i64 =
        sqlx::query_scalar("SELECT count(*) FROM currency_given_histories WHERE receiver_id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(records, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_response_edit_failure_does_not_repeat_issuance(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let discord = FakeDiscord::with_response_edit_error();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let mut payload = from_guild(
        money.user2,
        Some(json!(100)),
        money.user1,
        money.guild,
        DEFAULT_PERMISSIONS,
    );
    payload["id"] = json!("900000000000006001");
    assert_eq!(interaction(app.clone(), payload.clone()).await.status, 202);
    finished(&discord).await;
    assert_eq!(interaction(app, payload).await.status, 202);
    assert_acknowledgement(&discord);
    assert!(discord.response_edits().is_empty());
    assert!(discord.webhooks().is_empty());
    assert_eq!(discord.response_deletions(), 0);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 100
    );
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .unwrap()
            .pool_amount,
        Some(400)
    );
    let records: i64 =
        sqlx::query_scalar("SELECT count(*) FROM currency_given_histories WHERE receiver_id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(records, 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_database_failure_rolls_back_issuance_and_updates_the_private_response(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    sqlx::query("ALTER TABLE currency_given_histories ADD CONSTRAINT injected_issue_failure CHECK (amount <> 100)")
        .execute(&pool).await.unwrap();
    let response = issue(
        pool.clone(),
        from_guild(
            money.user2,
            Some(json!(100)),
            money.user1,
            money.guild,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;
    assert_error(
        &response,
        "発行結果を確認できませんでした。発行履歴を確認してください。",
    );
    assert_eq!(get_amount(&pool, money.user2, money.currency).await, before);
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .unwrap()
            .pool_amount,
        Some(500)
    );
    let records: i64 =
        sqlx::query_scalar("SELECT count(*) FROM currency_given_histories WHERE receiver_id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(records, 0);
}
