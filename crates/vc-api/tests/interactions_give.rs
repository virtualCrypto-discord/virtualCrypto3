//! The `give` command.
//!
//! Elixir has no test for this command — `Money.give/1` is only exercised through
//! `setup_money/1` — so these cases are additions rather than a port, and they
//! follow `Command.handle/4` and `Query.Issue.issue/3` directly.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    DEFAULT_PERMISSIONS, Response, currency_by_unit, fake, get_amount, interaction, setup_money,
    state,
};

const COLOR_OK: i64 = 0x0038_EA42;

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

fn give_data(receiver: i64, amount: Option<Value>) -> Value {
    let mut options = vec![json!({ "name": "user", "value": receiver.to_string() })];
    if let Some(amount) = amount {
        options.push(json!({ "name": "amount", "value": amount }));
    }

    json!({ "name": "give", "options": options })
}

fn from_guild(
    receiver: i64,
    amount: Option<Value>,
    sender: i64,
    guild_id: i64,
    permissions: &str,
) -> Value {
    support::from_guild(give_data(receiver, amount), sender, guild_id, permissions)
}

fn assert_error(response: &Response, content: &str) {
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({
            "type": 4,
            "data": {
                "tts": false,
                "flags": 64,
                "content": content,
                "allowed_mentions": { "parse": [] },
            },
        })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn give_issues_from_the_pool(pool: PgPool) {
    let money = setup_money(&pool).await;
    let receiver_before = get_amount(&pool, money.user2, money.currency).await;

    let response = interaction(
        router(pool.clone()),
        from_guild(
            money.user2,
            Some(json!(100)),
            money.user1,
            money.guild,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({
            "type": 4,
            "data": {
                "embeds": [{
                    "description": format!(
                        "✅ <@{}>へ**100** `{}`発行されました。\n残りの発行枠: **400** `{}`",
                        money.user2, money.unit, money.unit
                    ),
                    "color": COLOR_OK,
                }],
                "allowed_mentions": { "parse": [] },
            },
        })
    );

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
async fn give_without_an_amount_issues_the_whole_pool(pool: PgPool) {
    let money = setup_money(&pool).await;
    let receiver_before = get_amount(&pool, money.user2, money.currency).await;

    let response = interaction(
        router(pool.clone()),
        from_guild(
            money.user2,
            None,
            money.user1,
            money.guild,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["embeds"][0]["description"],
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
async fn give_more_than_the_pool_is_refused(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router(pool),
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
async fn give_needs_the_administrator_bit(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild(money.user2, Some(json!(100)), money.user1, money.guild, "0"),
    )
    .await;

    assert_error(&response, "エラー: 実行には管理者権限が必要です。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn give_in_a_direct_message_is_refused(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router(pool),
        support::execute_from_dm(give_data(money.user2, Some(json!(100))), money.user1),
    )
    .await;

    assert_error(&response, "エラー: DMでは実行できません。");
}
