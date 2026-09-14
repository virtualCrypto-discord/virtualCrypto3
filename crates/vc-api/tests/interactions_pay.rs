//! The `pay` command, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/pay_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{Response, execute_from_guild, fake, get_amount, interaction, setup_money, state};

const COLOR_OK: i64 = 0x38EA42;

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// `InteractionsControllerTest.Pay.Helper.pay_data/1`.
fn pay_data(receiver: i64, amount: Value, unit: &str) -> Value {
    json!({
        "name": "pay",
        "options": [
            { "name": "user", "value": receiver.to_string() },
            { "name": "amount", "value": amount },
            { "name": "unit", "value": unit },
        ],
    })
}

/// `Helper.from_guild/2`.
fn from_guild(receiver: i64, amount: Value, unit: &str, sender: i64) -> Value {
    execute_from_guild(pay_data(receiver, amount, unit), sender)
}

/// `Interactions.Pay.render/2` for `:error`, which every failure shares.
fn assert_error(response: &Response, content: &str) {
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"][0]["components"][0]["content"],
        json!(content),
    );

    assert_eq!(response.body["data"]["flags"], json!(32832));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_unit_is_reported(pool: PgPool) {
    let money = setup_money(&pool).await;
    let amount = get_amount(&pool, money.user1, money.currency).await;

    let response = interaction(
        router(pool),
        from_guild(money.user2, json!(amount), "void", money.user1),
    )
    .await;

    assert_error(&response, "エラー: 通貨は存在しません。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_positive_amount_is_reported(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild(money.user2, json!(-1), &money.unit, money.user1),
    )
    .await;

    assert_error(&response, "エラー: 不正な金額です。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payment_moves_the_amount_between_the_accounts(pool: PgPool) {
    let money = setup_money(&pool).await;
    let sender_before = get_amount(&pool, money.user1, money.currency).await;
    let receiver_before = get_amount(&pool, money.user2, money.currency).await;

    let response = interaction(
        router(pool.clone()),
        from_guild(money.user2, json!(20), &money.unit, money.user1),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"][0]["components"][0]["content"],
        json!(format!(
            "<@{}>から<@{}>へ**20** `{}`送金されました。",
            money.user1, money.user2, money.unit
        )),
    );

    assert_eq!(
        response.body["data"]["components"][0]["accent_color"],
        json!(COLOR_OK),
    );

    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        sender_before - 20
    );
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        receiver_before + 20
    );
}

/// The receiver has no account, so paying them creates one.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_an_unknown_receiver_creates_their_account(pool: PgPool) {
    let money = setup_money(&pool).await;
    let sender_before = get_amount(&pool, money.user1, money.currency).await;
    let receiver = -1;

    let response = interaction(
        router(pool.clone()),
        from_guild(receiver, json!(20), &money.unit, money.user1),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"][0]["components"][0]["content"],
        json!(format!(
            "<@{}>から<@{}>へ**20** `{}`送金されました。",
            money.user1, receiver, money.unit
        ))
    );

    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        sender_before - 20
    );
    assert_eq!(get_amount(&pool, receiver, money.currency).await, 20);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_more_than_the_balance_is_reported(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild(money.user2, json!(1_000_000), &money.unit, money.user1),
    )
    .await;

    assert_error(&response, "エラー: 通貨が不足しています。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_the_whole_balance_empties_the_account(pool: PgPool) {
    let money = setup_money(&pool).await;
    let sender_before = get_amount(&pool, money.user1, money.currency).await;
    let receiver_before = get_amount(&pool, money.user2, money.currency).await;

    let response = interaction(
        router(pool.clone()),
        from_guild(money.user2, json!(sender_before), &money.unit, money.user1),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"][0]["components"][0]["content"],
        json!(format!(
            "<@{}>から<@{}>へ**{}** `{}`送金されました。",
            money.user1, money.user2, sender_before, money.unit
        ))
    );

    assert_eq!(get_amount(&pool, money.user1, money.currency).await, 0);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        receiver_before + sender_before
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_one_more_than_the_balance_is_reported(pool: PgPool) {
    let money = setup_money(&pool).await;
    let amount = get_amount(&pool, money.user1, money.currency).await + 1;

    let response = interaction(
        router(pool),
        from_guild(money.user2, json!(amount), &money.unit, money.user1),
    )
    .await;

    assert_error(&response, "エラー: 通貨が不足しています。");
}

/// An amount Discord sends as a string is parsed the same way. This one is past
/// `2^53`, which is exactly the sort of value a client would send as a string,
/// and it is far more than the sender holds.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_amount_sent_as_a_string_is_parsed(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild(
            money.user2,
            json!("9007199254740992"),
            &money.unit,
            money.user1,
        ),
    )
    .await;

    assert_error(&response, "エラー: 通貨が不足しています。");
}
