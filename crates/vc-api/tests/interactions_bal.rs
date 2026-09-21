//! The `bal` command, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/bal_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{execute_from_guild, fake, interaction, setup_money, state};

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

fn bal_data() -> Value {
    json!({ "name": "bal", "options": [] })
}

/// What the one container says, which is everything this screen has to say.
fn texts(response: &Value) -> Vec<String> {
    response["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .filter_map(|child| child["content"].as_str().map(str::to_owned))
        .collect()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bal_lists_the_currency_the_user_holds(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), execute_from_guild(bal_data(), money.user1)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(4));
    assert_eq!(
        texts(&response.body),
        [
            "**所持通貨一覧** (1件)".to_string(),
            format!("**{}**\n199500 {}", money.name, money.unit),
        ]
    );
    assert_eq!(
        response.body["data"]["components"][0]["accent_color"],
        json!(0x6221ED),
    );

    assert_eq!(response.body["data"]["flags"], json!(32832),);
}

/// The list is ordered by unit, which for these two currencies is `n` then `w`.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bal_lists_every_currency_in_unit_order(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), execute_from_guild(bal_data(), money.user2)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response.body),
        [
            "**所持通貨一覧** (2件)".to_string(),
            format!("**{}**\n1000 {}", money.name, money.unit),
            format!("**{}**\n200000 {}", money.name2, money.unit2),
        ]
    );

    assert_eq!(response.body["data"]["flags"], json!(32832),);
}

/// A discord id with no account has no assets to join, so the command answers
/// with the empty list rather than creating anything.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bal_for_an_unknown_user_says_there_is_nothing(pool: PgPool) {
    setup_money(&pool).await;

    let response = interaction(router(pool), execute_from_guild(bal_data(), -1)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response.body), ["通貨を持っていません。"]);

    assert_eq!(response.body["data"]["flags"], json!(32832),);
}
