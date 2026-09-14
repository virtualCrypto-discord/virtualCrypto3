//! The `delete` command, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/delete_test.exs`.
//!
//! The submission of the modal this command answers with is a type 5
//! interaction, which is not implemented yet; that case is recorded in
//! docs/test-port.md.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{DEFAULT_PERMISSIONS, fake, interaction, setup_money, state};
use time::{Duration, OffsetDateTime};

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

fn delete_data() -> Value {
    json!({ "name": "delete" })
}

/// `execute_from_guild(delete_data(), user, guild)`: the command acts on the
/// guild's currency, so the tests point it at the one `setup_money` made.
fn from_guild(user: i64, guild_id: i64) -> Value {
    support::from_guild(delete_data(), user, guild_id, DEFAULT_PERMISSIONS)
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn delete_asks_for_confirmation(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild(money.user1, money.guild)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(9));
    assert_eq!(response.body["data"]["title"], json!("通貨の削除"));

    let required = format!("delete {}", money.unit);
    let field = &response.body["data"]["components"][0];

    assert_eq!(field["type"], json!(18));
    assert_eq!(
        field["label"],
        json!(format!("確認のため、「{required}」と入力してください。"))
    );

    let input = &field["component"];

    assert_eq!(input["type"], json!(4));
    assert_eq!(input["custom_id"], json!("confirm"));
    assert_eq!(input["style"], json!(1));
    assert_eq!(input["placeholder"], json!(required));
}

/// The Elixir test moves the clock past the window instead; ageing the row is
/// the same thing from the query's point of view.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn delete_outside_the_window_is_refused(pool: PgPool) {
    let money = setup_money(&pool).await;

    let aged = OffsetDateTime::now_utc() - Duration::days(73);
    let aged = time::PrimitiveDateTime::new(aged.date(), aged.time());
    support::set_currency_inserted_at(&pool, money.currency, aged).await;

    let response = interaction(router(pool), from_guild(money.user1, money.guild)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "components": [{
                "type": 10,
                "content": "エラー: 作成から72時間以上経過しているため削除できません。",
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

/// `delete_test.exs`'s "confirm delete": the typed unit is the confirmation, and
/// the currency and everything hanging off it go.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn confirming_the_modal_deletes_the_currency(pool: PgPool) {
    let money = setup_money(&pool).await;

    let payload = json!({
        "type": 5,
        "data": {
            "custom_id": vc_api::custom_id::encode(
                0,
                &vc_api::custom_id::ui::modal::confirm_currency_delete(),
            ),
            "components": [{
                "type": 18,
                "component": {
                    "custom_id": "confirm",
                    "value": format!("delete {}", money.unit),
                },
            }],
        },
        "member": {
            "user": { "id": money.user1.to_string() },
            "permissions": "18446744073709551615",
        },
        "guild_id": money.guild.to_string(),
    });

    let response = interaction(router(pool.clone()), payload).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "components": [{
                "type": 10,
                "content": "通貨を削除しました。",
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));

    assert!(
        support::currency_by_unit(&pool, &money.unit)
            .await
            .is_none(),
        "the currency is gone"
    );
}
