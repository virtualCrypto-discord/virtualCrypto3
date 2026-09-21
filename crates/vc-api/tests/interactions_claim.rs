//! The `claim make` and `claim show` commands, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/claim/claim_make_test.exs`
//! and `claim_show_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    ClaimSet, execute_from_guild, fake, get_amount, insert_claim, interaction, setup_claim, state,
};
use vc_api::claim_list::{ListOptions, Page, Position};
use vc_api::custom_id::ui::button::{
    Action, ListScope, claim_action, claim_action_single, claim_list,
};

const COLOR_ERROR: i64 = 0x00EA_3875;
const COLOR_BRAND: i64 = 6_431_213;
const COLOR_OK: i64 = 0x0038_EA42;

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// `claim_make_test.exs`'s own builder: `make` needs neither a guild nor
/// permissions, so its payload carries only the member's user id.
fn make_from_guild(claimant: i64, payer: i64, unit: &str, amount: Value) -> Value {
    json!({
        "type": 2,
        "data": {
            "name": "claim",
            "options": [{
                "name": "make",
                "options": [
                    { "name": "user", "value": payer.to_string() },
                    { "name": "unit", "value": unit },
                    { "name": "amount", "value": amount },
                ],
            }],
        },
        "member": { "user": { "id": claimant.to_string() } },
    })
}

/// `InteractionsControllerTest.Claim.Helper.patch_from_guild/3` with `show`.
fn show_from_guild(user: i64, id: i64) -> Value {
    execute_from_guild(
        json!({
            "name": "claim",
            "options": [{
                "name": "show",
                "options": [{ "name": "id", "value": id.to_string() }],
            }],
        }),
        user,
    )
}

/// Compare the parts of `Interactions.Claim.Show.render/1` the Elixir test
/// compares, since the buttons also carry a `custom_id` it does not pin.
fn assert_shown(response: &support::Response, claim_id: i64, claim: &ClaimSet, inserted_at: i64) {
    assert_eq!(response.status, 200, "body: {}", response.body);

    let money = &claim.money;
    let body = &response.body;

    assert_eq!(body["type"], json!(4));
    assert_eq!(body["data"]["flags"], json!(32832));
    assert_eq!(body["data"]["content"], json!(null));

    // The container holds the text and then the buttons, and it carries the accent both embeds
    // had.
    let container = &body["data"]["components"][0];
    assert_eq!(container["type"], json!(17));
    assert_eq!(container["accent_color"], json!(COLOR_BRAND));

    // The embed's title and its field, which were two keys and are one sentence now.
    assert_eq!(
        container["components"][0]["content"],
        json!(format!(
            "**請求**\n**📤📥{claim_id}**\n状態　: ⌛未決定\n請求額: **100** `{}`\n請求元: <@{}>\n請求先: <@{}>\n請求日: <t:{inserted_at}>",
            money.unit, money.user1, money.user1
        ))
    );

    // And the quotation, the same way.
    assert_eq!(
        container["components"][1]["content"],
        json!(format!(
            "**残高**\n**{}**: `200000{}` - `100{}` => `199900{}`",
            money.name, money.unit, money.unit, money.unit
        ))
    );

    let row = &container["components"][2];
    assert_eq!(row["type"], json!(1));

    let buttons = row["components"].as_array().expect("the buttons");
    assert_eq!(buttons.len(), 3);

    for (button, (style, emoji)) in buttons.iter().zip([(3, "✅"), (2, "❌"), (1, "🗑️")]) {
        assert_eq!(button["type"], json!(2));
        assert_eq!(button["style"], json!(style));
        assert_eq!(button["emoji"]["name"], json!(emoji));
        assert_eq!(button["disabled"], json!(false));
        assert!(
            button["custom_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "a button carries a custom_id"
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn make_creates_a_claim_and_reports_its_id(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(
        router(pool.clone()),
        make_from_guild(money.user2, money.user1, &money.unit, json!(100)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let description = response.body["data"]["components"][0]["components"][0]["content"]
        .as_str()
        .expect("a description")
        .to_string();
    assert_eq!(response.body["data"]["flags"], json!(32832));
    assert_eq!(response.body["type"], json!(4));

    let prefix = "請求id: ";
    let suffix = " で請求を受け付けました。";
    assert!(description.starts_with(prefix), "{description}");
    assert!(description.contains(suffix), "{description}");

    let claim_id: i64 = description[prefix.len()..description.find(suffix).expect("the suffix")]
        .parse()
        .expect("the claim id");
    assert!(
        description.ends_with(&format!("`/claim show id:{claim_id}`でご確認ください。")),
        "{description}"
    );

    let created = vc_core::claim::view(&pool, 1, claim_id)
        .await
        .expect("a lookup")
        .expect("the claim exists");

    assert_eq!(created.amount, Some(100));
    assert_eq!(created.currency.unit.as_deref(), Some(money.unit.as_str()));
    assert_eq!(created.claimant.discord_id, Some(money.user2));
    assert_eq!(created.payer.discord_id, Some(money.user1));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn make_rejects_a_non_positive_amount(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(
        router(pool),
        make_from_guild(money.user2, money.user1, &money.unit, json!(-100)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_ERROR,
            "components": [{
                "type": 10,
                "content": "**エラー**\n不正な金額です。1以上9223372036854775807以下である必要があります。",
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn make_rejects_an_unknown_unit(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(
        router(pool),
        make_from_guild(money.user2, money.user1, "void", json!(100)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_ERROR,
            "components": [{
                "type": 10,
                "content": "**エラー**\n指定された通貨は存在しません。",
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn show_reports_an_unknown_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(router(pool), show_from_guild(money.user1, -1)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_ERROR,
            "components": [{
                "type": 10,
                "content": "**エラー**\nそのidの請求は見つかりませんでした。",
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

/// c6 is a claim user1 made on themselves, so user2 is a party to nothing in it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn show_hides_a_claim_the_caller_is_not_part_of(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;
    let claim_id = claims.id(5);

    let response = interaction(router(pool), show_from_guild(money.user2, claim_id)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_ERROR,
            "components": [{
                "type": 10,
                "content": "**エラー**\nそのidの請求は見つかりませんでした。",
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

/// The caller is both parties, holds more than the claim asks, and so sees the
/// quotation and three enabled buttons.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn show_renders_a_pending_claim_with_its_actions(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;
    let claim_id = claims.id(5);

    let inserted_at = vc_core::claim::view(&pool, 1, claim_id)
        .await
        .expect("a lookup")
        .expect("the claim exists")
        .inserted_at
        .assume_utc()
        .unix_timestamp();

    let response = interaction(router(pool), show_from_guild(money.user1, claim_id)).await;

    assert_shown(&response, claim_id, &claims, inserted_at);
}

/// `InteractionsControllerTest.Claim.Helper.patch_from_guild/3`: a `claim
/// <action> id:<id>` command sent by `user`.
fn patch_from_guild(action: &str, id: i64, user: i64) -> Value {
    execute_from_guild(
        json!({
            "name": "claim",
            "options": [{
                "name": action,
                "options": [{ "name": "id", "value": id.to_string() }],
            }],
        }),
        user,
    )
}

async fn patch(pool: PgPool, action: &str, id: i64, user: i64) -> support::Response {
    interaction(router(pool), patch_from_guild(action, id, user)).await
}

/// `Helper.assert_discord_message/2`: the claim command's error embed, which
/// carries no `allowed_mentions`.
fn assert_message(response: &support::Response, message: &str) {
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_ERROR,
            "components": [{
                "type": 10,
                "content": format!("**エラー**\n{}", message),
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

/// `Interactions.Claim.render/1` for a transition the caller was allowed to make.
fn assert_action_result(response: &support::Response, claim_id: i64, result: &str) {
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_OK,
            "components": [{
                "type": 10,
                "content": format!("id: {claim_id}の請求を{result}"),
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

const INVALID_OPERATOR: &str = "この請求に対してこの操作を行う権限がありません。";
const INVALID_STATUS: &str = "この請求に対してこの操作を行うことは出来ません。";
const NOT_FOUND: &str = "そのidの請求は見つかりませんでした。";
const NOT_ENOUGH: &str = "お金が足りません。";

/// c1 is pending, so the payer can approve it and the money moves.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_pays_the_claimant(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;
    let claim_id = claims.id(0);

    let claimant_before = get_amount(&pool, money.user1, money.currency).await;
    let payer_before = get_amount(&pool, money.user2, money.currency).await;

    let response = patch(pool.clone(), "approve", claim_id, money.user2).await;

    assert_action_result(&response, claim_id, "承諾し、支払いました。");

    let claim = vc_core::claim::view(&pool, 1, claim_id)
        .await
        .expect("a lookup")
        .expect("the claim exists");
    assert_eq!(claim.status.as_deref(), Some("approved"));

    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        claimant_before + 500
    );
    // The payer is left with nothing, and the assets trigger drops the row.
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        payer_before - 500
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_the_claimant(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", claims.id(0), money.user1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_an_unrelated_user(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "approve", claims.id(0), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

/// c2 asks for far more than its payer holds, and the payer's own approval is
/// what uncovers it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_reports_a_payer_who_cannot_cover_the_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", claims.id(1), money.user1).await;

    assert_message(&response, NOT_ENOUGH);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_the_claimant_of_an_unaffordable_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", claims.id(1), money.user2).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_an_unrelated_user_of_an_unaffordable_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "approve", claims.id(1), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_reports_an_already_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", claims.id(2), money.user2).await;

    assert_message(&response, INVALID_STATUS);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_the_claimant_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", claims.id(2), money.user1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_an_unrelated_user_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "approve", claims.id(2), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_reports_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", claims.id(3), money.user2).await;

    assert_message(&response, INVALID_STATUS);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_the_claimant_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", claims.id(3), money.user1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_an_unrelated_user_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "approve", claims.id(3), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_reports_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", claims.id(4), money.user2).await;

    assert_message(&response, INVALID_STATUS);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_the_claimant_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", claims.id(4), money.user1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_rejects_an_unrelated_user_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "approve", claims.id(4), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approve_reports_an_unknown_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "approve", -1, money.user1).await;

    assert_message(&response, NOT_FOUND);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_leaves_the_money_where_it_is(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;
    let claim_id = claims.id(0);

    let claimant_before = get_amount(&pool, money.user1, money.currency).await;
    let payer_before = get_amount(&pool, money.user2, money.currency).await;

    let response = patch(pool.clone(), "deny", claim_id, money.user2).await;

    assert_action_result(&response, claim_id, "拒否しました。");

    let claim = vc_core::claim::view(&pool, 1, claim_id)
        .await
        .expect("a lookup")
        .expect("the claim exists");
    assert_eq!(claim.status.as_deref(), Some("denied"));

    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        claimant_before
    );
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        payer_before
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_rejects_the_claimant(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "deny", claims.id(0), money.user1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_rejects_an_unrelated_user(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "deny", claims.id(0), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_reports_an_already_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "deny", claims.id(2), money.user2).await;

    assert_message(&response, INVALID_STATUS);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_rejects_the_claimant_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "deny", claims.id(2), money.user1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_rejects_an_unrelated_user_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "deny", claims.id(2), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_reports_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "deny", claims.id(3), money.user2).await;

    assert_message(&response, INVALID_STATUS);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_rejects_the_claimant_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "deny", claims.id(3), money.user1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_rejects_an_unrelated_user_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "deny", claims.id(3), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_reports_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "deny", claims.id(4), money.user2).await;

    assert_message(&response, INVALID_STATUS);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_rejects_the_claimant_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "deny", claims.id(4), money.user1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_rejects_an_unrelated_user_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "deny", claims.id(4), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deny_reports_an_unknown_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "deny", -1, money.user1).await;

    assert_message(&response, NOT_FOUND);
}

/// Cancelling is the claimant's move, the mirror of approving.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_is_the_claimants_move(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let claim_id = claims.id(0);

    let response = patch(pool.clone(), "cancel", claim_id, claims.money.user1).await;

    assert_action_result(&response, claim_id, "キャンセルしました。");

    let claim = vc_core::claim::view(&pool, 1, claim_id)
        .await
        .expect("a lookup")
        .expect("the claim exists");
    assert_eq!(claim.status.as_deref(), Some("canceled"));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_rejects_the_payer(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "cancel", claims.id(0), money.user2).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_rejects_an_unrelated_user(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "cancel", claims.id(0), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_reports_an_already_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "cancel", claims.id(2), money.user1).await;

    assert_message(&response, INVALID_STATUS);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_rejects_the_payer_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "cancel", claims.id(2), money.user2).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_rejects_an_unrelated_user_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "cancel", claims.id(2), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_reports_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "cancel", claims.id(3), money.user1).await;

    assert_message(&response, INVALID_STATUS);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_rejects_the_payer_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "cancel", claims.id(3), money.user2).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_rejects_an_unrelated_user_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "cancel", claims.id(3), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_reports_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "cancel", claims.id(4), money.user1).await;

    assert_message(&response, INVALID_STATUS);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_rejects_the_payer_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "cancel", claims.id(4), money.user2).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_rejects_an_unrelated_user_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    let response = patch(pool, "cancel", claims.id(4), -1).await;

    assert_message(&response, INVALID_OPERATOR);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancel_reports_an_unknown_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = patch(pool, "cancel", -1, money.user1).await;

    assert_message(&response, NOT_FOUND);
}

/// `InteractionsControllerTest.Claim.Helper.list_from_guild/1`.
fn list_from_guild(user: i64) -> Value {
    execute_from_guild(
        json!({
            "name": "claim",
            "options": [{ "name": "list", "options": [] }],
        }),
        user,
    )
}

/// The options the command opens with: pending only, first page, no filter.
fn list_options(position: Position) -> ListOptions {
    ListOptions {
        pending: true,
        approved: false,
        denied: false,
        canceled: false,
        position,
        page: Page::Number(1),
        related_user: None,
    }
}

fn scope(position: Position) -> ListScope {
    match position {
        Position::All => ListScope::All,
        Position::Received => ListScope::Received,
        Position::Claimed => ListScope::Claimed,
    }
}

/// `Listing.custom_id/4`, so a pagination button can be compared exactly.
fn page_custom_id(k: u8, position: Position, page: Page, options: &ListOptions) -> String {
    let mut payload = claim_list(scope(position)).to_vec();
    payload.extend_from_slice(&ListOptions { page, ..*options }.encode());

    vc_api::custom_id::encode(k, &payload)
}

fn claim_icon(me: i64, claim: &vc_core::claim::ClaimView) -> &'static str {
    let claimant = claim.claimant.discord_id == Some(me);
    let payer = claim.payer.discord_id == Some(me);

    match (claimant, payer) {
        (true, true) => "📤📥",
        (true, false) => "📤",
        (false, true) => "📥",
        (false, false) => "",
    }
}

/// The page all of these commands open on: pending claims only, newest first.
async fn first_page(pool: &PgPool, account: i32) -> vc_core::claim::ClaimPage {
    vc_core::claim::list_page(
        pool,
        account,
        &["pending".to_string()],
        vc_core::claim::SrFilter::All,
        None,
        1,
        5,
    )
    .await
    .expect("the page")
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn list_narrows_to_the_user_the_filter_names(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    // The same list twice: once as the command opens it, and once with one of the
    // two names on it. Two of the caller's three pending claims name the other
    // user; the third is between the caller and themselves.
    let every = interaction(router(pool.clone()), list_from_guild(money.user1)).await;
    let narrowed = interaction(
        router(pool.clone()),
        execute_from_guild(
            json!({
                "name": "claim",
                "options": [{
                    "name": "list",
                    "options": [{ "name": "user", "value": money.user2.to_string() }],
                }],
            }),
            money.user1,
        ),
    )
    .await;

    assert_eq!(every.status, 200, "body: {}", every.body);
    assert_eq!(narrowed.status, 200, "body: {}", narrowed.body);

    assert_eq!(
        claims_on(every.body["data"].to_string().as_str()),
        3,
        "the caller's pending claims: {}",
        every.body["data"]
    );
    assert_eq!(
        claims_on(narrowed.body["data"].to_string().as_str()),
        2,
        "the ones that name the other user: {}",
        narrowed.body["data"]
    );
}

/// How many claims a rendered list holds: one line per claim says its state.
fn claims_on(rendered: &str) -> usize {
    rendered.matches("状態　:").count()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn list_reports_an_empty_page(pool: PgPool) {
    setup_claim(&pool).await;

    let response = interaction(router(pool), list_from_guild(-1)).await;

    let options = list_options(Position::All);
    let reload = page_custom_id(4, Position::All, Page::Number(1), &options);

    assert_eq!(response.status, 200, "body: {}", response.body);
    // What the page says, and then the row that changes it: the title and the empty state are
    // Text Displays where an embed's title and description were.
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_BRAND,
            "components": [
                { "type": 10, "content": "**請求一覧(all)**" },
                { "type": 10, "content": "表示する内容がありません。" },
                {
                    "type": 1,
                    "components": [
                        { "type": 2, "style": 2, "emoji": { "name": "⏪" }, "custom_id": "disabled-0", "disabled": true },
                        { "type": 2, "style": 2, "emoji": { "name": "⏮️" }, "custom_id": "disabled-1", "disabled": true },
                        { "type": 2, "style": 2, "emoji": { "name": "⏭️" }, "custom_id": "disabled-2", "disabled": true },
                        { "type": 2, "style": 2, "emoji": { "name": "⏩" }, "custom_id": "disabled-3", "disabled": true },
                        { "type": 2, "style": 2, "emoji": { "name": "🔄" }, "custom_id": reload },
                    ],
                },
            ],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn list_renders_the_first_page(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(router(pool.clone()), list_from_guild(money.user1)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(4));
    assert_eq!(response.body["data"]["flags"], json!(32832));

    let page = first_page(&pool, 1).await;
    assert_eq!(
        page.claims.len(),
        3,
        "user1 is party to three pending claims"
    );

    let expected: Vec<Value> = page
        .claims
        .iter()
        .map(|claim| {
            json!({
                "type": 10,
                "content": format!(
                    "**{}{}**\n状態　: ⌛未決定\n請求額: **{}** `{}`\n請求元: <@{}>\n請求先: <@{}>\n請求日: <t:{}>",
                    claim_icon(money.user1, claim),
                    claim.id,
                    claim.amount.unwrap_or_default(),
                    claim.currency.unit.clone().unwrap_or_default(),
                    claim.claimant.discord_id.unwrap_or_default(),
                    claim.payer.discord_id.unwrap_or_default(),
                    claim.inserted_at.assume_utc().unix_timestamp(),
                ),
            })
        })
        .collect();

    // The title, then each claim with the buttons its own row offers. The rows are read from
    // the front for what was said and from the back for the controls, because how many rows
    // there are depends on the page.
    let container = &response.body["data"]["components"][0];
    assert_eq!(container["type"], json!(17));
    assert_eq!(container["accent_color"], json!(COLOR_BRAND));
    let children = container["components"].as_array().expect("the children");

    assert_eq!(children[0]["content"], json!("**請求一覧(all)**"));

    let said: Vec<Value> = children
        .iter()
        .filter(|child| child["type"] == json!(10))
        .cloned()
        .collect();

    assert_eq!(
        &said[1..1 + expected.len()],
        expected.as_slice(),
        "the list: {container}"
    );

    // And a pending claim's buttons come with the claim they belong to: the same three the
    // claim's own screen offers, under the same rules — the approval is the payer's and only
    // with the money, the refusal is the payer's, and taking it back is the claimant's.
    let balances = vc_core::balance::for_discord_user(&pool, money.user1)
        .await
        .expect("the caller's balances");

    let mut read = 1;
    let mut offered = children.iter().filter(|child| child["type"] == json!(1));

    for claim in &page.claims {
        let row = children.get(read + 1).expect("the claim's own row").clone();

        assert_eq!(row["type"], json!(1), "after its claim: {container}");
        assert_eq!(&row, offered.next().expect("a row"));

        let unit = claim.currency.unit.clone().unwrap_or_default();
        let current = balances
            .iter()
            .find(|balance| balance.unit == unit)
            .map(|balance| balance.amount)
            .unwrap_or(0);
        let payer = claim.payer.discord_id == Some(money.user1);
        let claimant = claim.claimant.discord_id == Some(money.user1);

        let buttons = row["components"].as_array().expect("the buttons");

        assert_eq!(buttons.len(), 3);
        assert_eq!(buttons[0]["emoji"]["name"], json!("✅"));
        assert_eq!(buttons[0]["style"], json!(3));
        assert_eq!(
            buttons[0]["disabled"],
            json!(!(payer && current >= claim.amount.unwrap_or_default())),
            "an approval needs the money"
        );
        assert_eq!(buttons[1]["emoji"]["name"], json!("❌"));
        assert_eq!(buttons[1]["style"], json!(2));
        assert_eq!(buttons[1]["disabled"], json!(!payer));
        assert_eq!(buttons[2]["emoji"]["name"], json!("🗑️"));
        assert_eq!(buttons[2]["disabled"], json!(!claimant));

        read += 2;
    }

    let options = list_options(Position::All);

    // The pagination row is the last thing on the screen: the claims above carry their own
    // buttons, so nothing else follows them.
    assert_eq!(
        container["components"][container["components"].as_array().expect("children").len() - 1]["components"],
        json!([
            { "type": 2, "style": 2, "emoji": { "name": "⏪" }, "custom_id": "disabled-0", "disabled": true },
            { "type": 2, "style": 2, "emoji": { "name": "⏮️" }, "custom_id": "disabled-1", "disabled": true },
            { "type": 2, "style": 2, "emoji": { "name": "⏭️" }, "custom_id": "disabled-2", "disabled": true },
            { "type": 2, "style": 2, "emoji": { "name": "⏩" }, "custom_id": "disabled-3", "disabled": true },
            {
                "type": 2,
                "style": 2,
                "emoji": { "name": "🔄" },
                "custom_id": page_custom_id(4, Position::All, Page::Number(1), &options),
            },
        ])
    );
}

/// The button path's own wording, which differs from the command path's.
const BUTTON_UNAUTHORIZED: &str = "エラー: この請求に対してこの操作を行う権限がありません。";
const BUTTON_ALREADY_PROCESSED: &str = "エラー: 処理しようとした請求はすでに処理済みです。";
const BUTTON_NO_MONEY: &str = "エラー: お金が足りません。";
const BUTTON_NOT_FOUND: &str = "エラー: そのidの請求は見つかりませんでした。";

/// `InteractionsControllerTest.Claim.List.Helper.action_data/2`: a `type 3`
/// button press, carrying the token the follow-up is posted to.
fn action_data(custom_id: String, user: i64) -> Value {
    json!({
        "id": "123456789012345678",
        "type": 3,
        "data": { "custom_id": custom_id, "component_type": 2 },
        "member": {
            "user": { "id": user.to_string() },
            "permissions": "18446744073709551615",
        },
        "token": "discord_interaction_token",
        "application_id": "1234578901234567",
        "guild_id": "494780225280802817",
    })
}

/// `Helper.test_common/3`'s payload: the action, the list's options, and the
/// claims it applies to.
fn action_custom_id(action: Action, ids: &[i64]) -> String {
    let mut payload = claim_action(action).to_vec();
    payload.extend_from_slice(&list_options(Position::All).encode());
    payload.extend_from_slice(&vc_api::claim_list::encode_claim_ids(ids));

    vc_api::custom_id::encode(0, &payload)
}

/// Presses the button and reads back the follow-up body, which is where the
/// outcome is reported.
async fn press(
    api: &std::sync::Arc<support::FakeDiscord>,
    pool: PgPool,
    action: Action,
    ids: &[i64],
    user: i64,
) -> (support::Response, Value) {
    let response = interaction(
        vc_api::router(state(pool, api.clone())),
        action_data(action_custom_id(action, ids), user),
    )
    .await;

    assert_eq!(response.status, 202, "{}", response.body);
    assert_eq!(response.body, Value::Null);
    // Existing rendering assertions inspect the response delivered through the
    // callback now; the incoming request has only an empty acknowledgement.
    let response = support::Response {
        status: response.status,
        headers: response.headers,
        body: api
            .callbacks()
            .pop()
            .expect("an initial response was posted"),
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), api.followup_finished())
        .await
        .expect("follow-up completed");
    let mut posted = api.webhooks();
    let body = posted.pop().expect("a follow-up was posted");

    (response, body)
}

async fn assert_action_error(pool: PgPool, action: Action, claim: i64, user: i64, content: &str) {
    let api = fake();
    let (response, body) = press(&api, pool, action, &[claim], user).await;

    assert_eq!(response.status, 202, "body: {}", response.body);
    assert_eq!(
        body["components"],
        json!([{ "type": 17, "components": [{ "type": 10, "content": content }] }])
    );
    assert_eq!(body["flags"], json!(32832));
}

/// The one case in each file that carries the action through: the money moves,
/// the follow-up names the claim, and the list is redrawn for whoever pressed.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pressing_approve_pays_the_claimant(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;
    let claim_id = claims.id(0);

    let claimant_before = get_amount(&pool, money.user1, money.currency).await;
    let payer_before = get_amount(&pool, money.user2, money.currency).await;

    let api = fake();
    let (response, body) = press(
        &api,
        pool.clone(),
        Action::Approve,
        &[claim_id],
        money.user2,
    )
    .await;

    assert_eq!(
        body["components"],
        json!([{ "type": 17, "components": [{ "type": 10, "content": format!("id: `{claim_id}` の請求を承諾し、支払いました。") }] }])
    );
    assert_eq!(body["flags"], json!(32832));

    // The press answers with the list redrawn, not with the outcome.
    assert_eq!(response.status, 202, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(7));
    assert_eq!(response.body["data"]["flags"], json!(32832));
    assert_eq!(
        response.body["data"]["components"][0]["components"][0]["content"],
        json!("**請求一覧(all)**")
    );

    // Only user2's other claim is left, so the page holds one row.
    let remaining = first_page(&pool, 2).await;
    assert_eq!(remaining.claims.len(), 1);
    assert_eq!(remaining.claims[0].id, claims.id(1));
    // The pagination row is the last thing on the screen: the claim above carries its own
    // buttons, and nothing follows them.
    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");
    let last = children.last().expect("a row");

    assert_eq!(last["type"], json!(1));
    assert_eq!(last["components"][4]["emoji"]["name"], json!("🔄"));

    let claim = vc_core::claim::view(&pool, 1, claim_id)
        .await
        .expect("a lookup")
        .expect("the claim exists");
    assert_eq!(claim.status.as_deref(), Some("approved"));

    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        claimant_before + 500
    );
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        payer_before - 500
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_the_claimant(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Approve,
        claims.id(0),
        money.user1,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_an_unrelated_user(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Approve, claims.id(0), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_reports_a_payer_who_cannot_cover_the_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Approve,
        claims.id(1),
        money.user1,
        BUTTON_NO_MONEY,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_the_claimant_of_an_unaffordable_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Approve,
        claims.id(1),
        money.user2,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_an_unrelated_user_of_an_unaffordable_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Approve, claims.id(1), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_reports_an_already_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Approve,
        claims.id(2),
        money.user2,
        BUTTON_ALREADY_PROCESSED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_the_claimant_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Approve,
        claims.id(2),
        money.user1,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_an_unrelated_user_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Approve, claims.id(2), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_reports_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Approve,
        claims.id(3),
        money.user2,
        BUTTON_ALREADY_PROCESSED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_the_claimant_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Approve,
        claims.id(3),
        money.user1,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_an_unrelated_user_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Approve, claims.id(3), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_reports_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Approve,
        claims.id(4),
        money.user2,
        BUTTON_ALREADY_PROCESSED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_the_claimant_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Approve,
        claims.id(4),
        money.user1,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_rejects_an_unrelated_user_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Approve, claims.id(4), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_approve_reports_an_unknown_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(pool, Action::Approve, 0, money.user1, BUTTON_NOT_FOUND).await;
}

/// Denying is the payer's move and leaves the money alone.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pressing_deny_leaves_the_money_where_it_is(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;
    let claim_id = claims.id(0);

    let claimant_before = get_amount(&pool, money.user1, money.currency).await;
    let payer_before = get_amount(&pool, money.user2, money.currency).await;

    let api = fake();
    let (response, body) = press(&api, pool.clone(), Action::Deny, &[claim_id], money.user2).await;

    assert_eq!(response.status, 202, "body: {}", response.body);
    assert_eq!(
        body["components"],
        json!([{ "type": 17, "components": [{ "type": 10, "content": format!("id: `{claim_id}` の請求を拒否しました。") }] }])
    );
    assert_eq!(body["flags"], json!(32832));

    let claim = vc_core::claim::view(&pool, 1, claim_id)
        .await
        .expect("a lookup")
        .expect("the claim exists");
    assert_eq!(claim.status.as_deref(), Some("denied"));

    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        claimant_before
    );
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        payer_before
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_rejects_the_claimant(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Deny,
        claims.id(0),
        money.user1,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_rejects_an_unrelated_user(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Deny, claims.id(0), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_reports_an_already_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Deny,
        claims.id(2),
        money.user2,
        BUTTON_ALREADY_PROCESSED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_rejects_the_claimant_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Deny,
        claims.id(2),
        money.user1,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_rejects_an_unrelated_user_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Deny, claims.id(2), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_reports_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Deny,
        claims.id(3),
        money.user2,
        BUTTON_ALREADY_PROCESSED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_rejects_the_claimant_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Deny,
        claims.id(3),
        money.user1,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_rejects_an_unrelated_user_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Deny, claims.id(3), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_reports_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Deny,
        claims.id(4),
        money.user2,
        BUTTON_ALREADY_PROCESSED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_rejects_the_claimant_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Deny,
        claims.id(4),
        money.user1,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_rejects_an_unrelated_user_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Deny, claims.id(4), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_deny_reports_an_unknown_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(pool, Action::Deny, 0, money.user1, BUTTON_NOT_FOUND).await;
}

/// Cancelling is the claimant's move, the mirror of denying.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pressing_cancel_is_the_claimants_move(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let claim_id = claims.id(0);

    let api = fake();
    let (response, body) = press(
        &api,
        pool.clone(),
        Action::Cancel,
        &[claim_id],
        claims.money.user1,
    )
    .await;

    assert_eq!(response.status, 202, "body: {}", response.body);
    assert_eq!(
        body["components"],
        json!([{ "type": 17, "components": [{ "type": 10, "content": format!("id: `{claim_id}` の請求をキャンセルしました。") }] }])
    );
    assert_eq!(body["flags"], json!(32832));

    let claim = vc_core::claim::view(&pool, 1, claim_id)
        .await
        .expect("a lookup")
        .expect("the claim exists");
    assert_eq!(claim.status.as_deref(), Some("canceled"));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_rejects_the_payer(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Cancel,
        claims.id(0),
        money.user2,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_rejects_an_unrelated_user(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Cancel, claims.id(0), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_reports_an_already_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Cancel,
        claims.id(2),
        money.user1,
        BUTTON_ALREADY_PROCESSED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_rejects_the_payer_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Cancel,
        claims.id(2),
        money.user2,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_rejects_an_unrelated_user_of_an_approved_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Cancel, claims.id(2), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_reports_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Cancel,
        claims.id(3),
        money.user1,
        BUTTON_ALREADY_PROCESSED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_rejects_the_payer_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Cancel,
        claims.id(3),
        money.user2,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_rejects_an_unrelated_user_of_a_denied_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Cancel, claims.id(3), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_reports_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Cancel,
        claims.id(4),
        money.user1,
        BUTTON_ALREADY_PROCESSED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_rejects_the_payer_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(
        pool,
        Action::Cancel,
        claims.id(4),
        money.user2,
        BUTTON_UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_rejects_an_unrelated_user_of_a_canceled_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    assert_action_error(pool, Action::Cancel, claims.id(4), -1, BUTTON_UNAUTHORIZED).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn button_cancel_reports_an_unknown_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    assert_action_error(pool, Action::Cancel, 0, money.user1, BUTTON_NOT_FOUND).await;
}

/// The row's 🔄: it draws the page it is on, which is what a person presses when the
/// claim in front of them was decided somewhere else.
///
/// All five of the row's buttons answer through `[:claim, :list, position]`, and that
/// clause was the one this handler did not have: every one of them was an internal error,
/// which Discord shows as a failed interaction.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pressing_the_reload_button_draws_the_page_again(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let api = fake();
    let options = list_options(Position::All);
    let response = interaction(
        vc_api::router(state(pool.clone(), api.clone())),
        action_data(
            page_custom_id(4, Position::All, Page::Number(1), &options),
            money.user1,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["type"],
        json!(7),
        "a redraw edits the message"
    );
    // The caller's three pending claims, which is the page a reload of page one is.
    assert_eq!(claims_on(response.body["data"].to_string().as_str()), 3);
    assert_eq!(
        response.body["data"]["components"][0]["components"][0]["content"],
        json!("**請求一覧(all)**")
    );
    assert!(
        api.webhooks().is_empty(),
        "a redraw tells nobody: {:?}",
        api.webhooks()
    );
}

/// And the ⏭️: it draws the page the button names rather than the one the message was
/// showing, which is what makes a list longer than a page readable.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pressing_the_next_arrow_draws_the_next_page(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    // Four more of the caller's pending claims, so their pending list is two pages.
    for id in 7..=10 {
        insert_claim(&pool, id, 100, "pending", 1, 2, claims.money.currency).await;
    }

    let api = fake();
    let options = list_options(Position::All);
    let response = interaction(
        vc_api::router(state(pool.clone(), api.clone())),
        action_data(
            page_custom_id(2, Position::All, Page::Number(2), &options),
            claims.money.user1,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(7));
    assert_eq!(
        claims_on(response.body["data"].to_string().as_str()),
        2,
        "the two the first page left: {}",
        response.body
    );

    // The row follows the page it drew: ⏪ has somewhere to go and ⏭️ does not.
    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");
    let row = children.last().expect("the pagination row");

    assert_ne!(row["components"][0]["custom_id"], json!("disabled-0"));
    assert_eq!(row["components"][2]["custom_id"], json!("disabled-2"));

    // And the two ways back, pressed: each carries the page it moves to beside the options
    // the screen was showing, which is what makes them draw page one again.
    for arrow in [0, 1] {
        let back = interaction(
            vc_api::router(state(pool.clone(), fake())),
            action_data(
                page_custom_id(arrow, Position::All, Page::Number(1), &options),
                claims.money.user1,
            ),
        )
        .await;

        assert_eq!(back.status, 200, "body: {}", back.body);
        assert_eq!(back.body["type"], json!(7), "a redraw: {}", back.body);
        assert_eq!(
            claims_on(back.body["data"].to_string().as_str()),
            5,
            "page one holds five: {}",
            back.body
        );
    }
}

/// And the ⏩, whose id is the one this list writes differently: a page number nobody knows
/// yet — `:last` — rather than the page itself. The handler counts the pages for it, which is
/// a path no other arrow takes, so pressing it is the only way to know it draws the last page
/// rather than the first.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pressing_the_last_arrow_draws_the_last_page(pool: PgPool) {
    let claims = setup_claim(&pool).await;

    // Four more of the caller's pending claims: seven of them, and the list holds five.
    for id in 7..=10 {
        insert_claim(&pool, id, 100, "pending", 1, 2, claims.money.currency).await;
    }

    let api = fake();
    let options = list_options(Position::All);
    let response = interaction(
        vc_api::router(state(pool.clone(), api.clone())),
        action_data(
            page_custom_id(3, Position::All, Page::Last, &options),
            claims.money.user1,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(7));

    // The two the first page left, which is what says this is page two and not page one.
    assert_eq!(
        claims_on(response.body["data"].to_string().as_str()),
        2,
        "the last page, not the first: {}",
        response.body
    );

    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");
    let row = children.last().expect("the pagination row");

    assert_ne!(row["components"][0]["custom_id"], json!("disabled-0"));
    assert_eq!(row["components"][2]["custom_id"], json!("disabled-2"));
    assert_eq!(row["components"][3]["custom_id"], json!("disabled-3"));
}

/// `Show.action_custom_id/3`: the claim's own screen carries its id after the action.
fn single_action_custom_id(action: Action, claim_id: i64) -> String {
    let mut payload = claim_action_single(action).to_vec();
    payload.extend_from_slice(&claim_id.to_be_bytes());

    vc_api::custom_id::encode(1, &payload)
}

/// A claim's own screen answers on itself: the button that was pressed moves the claim,
/// and the message it was on comes back saying so and showing the claim it changed.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pressing_the_show_screens_approve_updates_the_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;
    let claim_id = claims.id(0);

    let api = fake();
    let response = interaction(
        vc_api::router(state(pool.clone(), api.clone())),
        action_data(
            single_action_custom_id(Action::Approve, claim_id),
            money.user2,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(7), "the screen edits itself");

    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");

    assert_eq!(
        children[0]["content"],
        json!(format!("id: {claim_id}の請求を承諾し、支払いました。"))
    );
    assert!(
        children[1]["content"]
            .as_str()
            .is_some_and(|claim| claim.contains("✅支払い済み")),
        "the claim it changed: {}",
        response.body
    );
    assert_eq!(
        children.len(),
        2,
        "a decided claim offers no buttons: {}",
        response.body
    );
    assert!(api.webhooks().is_empty(), "this screen answers with itself");
}

/// A refusal is the error screen the command answers with, and the claim is left alone.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pressing_the_show_screens_approve_as_the_claimant_is_refused(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(
        vc_api::router(state(pool.clone(), fake())),
        action_data(
            single_action_custom_id(Action::Approve, claims.id(0)),
            money.user1,
        ),
    )
    .await;

    assert_message(&response, INVALID_OPERATOR);

    let claim = vc_core::claim::view(&pool, 1, claims.id(0))
        .await
        .expect("a lookup")
        .expect("the claim exists");
    assert_eq!(claim.status.as_deref(), Some("pending"));
}

/// A notification outage cannot turn an already-paid claim into a failed interaction.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn followup_failure_does_not_fail_the_paid_claim_response(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let id = claims.id(0);
    let api = support::FakeDiscord::with_followup(true, None);
    let before = get_amount(&pool, claims.money.user2, claims.money.currency).await;
    let response = interaction(
        vc_api::router(state(pool.clone(), api.clone())),
        action_data(action_custom_id(Action::Approve, &[id]), claims.money.user2),
    )
    .await;
    assert_eq!(response.status, 202);
    tokio::time::timeout(std::time::Duration::from_secs(2), api.followup_finished())
        .await
        .unwrap();
    assert_eq!(api.callbacks().len(), 1);
    assert_eq!(api.callbacks()[0]["type"], 7);
    assert!(api.webhooks().is_empty());
    let claim = vc_core::claim::view(&pool, 1, id).await.unwrap().unwrap();
    assert_eq!(claim.status.as_deref(), Some("approved"));
    assert_eq!(
        get_amount(&pool, claims.money.user2, claims.money.currency).await,
        before - 500
    );
}

/// Hold delivery indefinitely: neither the callback nor the HTTP acknowledgement waits for it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn slow_followup_does_not_block_initial_response(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let api = support::FakeDiscord::with_followup(false, Some(gate.clone()));
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        interaction(
            vc_api::router(state(pool, api.clone())),
            action_data(
                action_custom_id(Action::Approve, &[claims.id(0)]),
                claims.money.user2,
            ),
        ),
    )
    .await
    .expect("initial response must not wait for delivery");
    assert_eq!(response.status, 202);
    tokio::time::timeout(std::time::Duration::from_secs(2), api.followup_started())
        .await
        .unwrap();
    assert_eq!(api.callbacks().len(), 1);
    assert_eq!(api.callbacks()[0]["type"], 7);
    assert!(api.webhooks().is_empty());
    gate.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(2), api.followup_finished())
        .await
        .unwrap();
    assert_eq!(api.webhooks().len(), 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_initial_callback_does_not_send_a_followup(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let api = support::FakeDiscord::with_callback_error();
    let response = interaction(
        vc_api::router(state(pool, api.clone())),
        action_data(
            action_custom_id(Action::Deny, &[claims.id(0)]),
            claims.money.user2,
        ),
    )
    .await;
    assert_eq!(response.status, 500);
    assert!(api.callbacks().is_empty());
    assert!(api.webhooks().is_empty());
}
