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

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bal_lists_the_currency_the_user_holds(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), execute_from_guild(bal_data(), money.user1)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(4));
    assert_eq!(
        response.body["data"]["components"][0]["components"][0]["content"],
        json!(format!(
            "所持通貨一覧\n```yaml\n{}: 199500 {}\n```",
            money.name, money.unit
        )),
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
        response.body["data"]["components"][0]["components"][0]["content"],
        json!(format!(
            "所持通貨一覧\n```yaml\n{}: 1000 {}\n{}: 200000 {}\n```",
            money.name, money.unit, money.name2, money.unit2
        )),
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
    assert_eq!(
        response.body["data"]["components"][0]["components"][0]["content"],
        json!("所持通貨一覧\n```\n通貨を持っていません。\n```"),
    );

    assert_eq!(response.body["data"]["flags"], json!(32832),);
}
