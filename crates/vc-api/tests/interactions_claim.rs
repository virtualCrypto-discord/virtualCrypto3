//! The `claim make` and `claim show` commands, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/claim/claim_make_test.exs`
//! and `claim_show_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{ClaimSet, execute_from_guild, fake, get_amount, interaction, setup_claim, state};
use vc_api::claim_list::{ListOptions, Page, Position};
use vc_api::custom_id::ui::button::{Action, ListScope, claim_action, claim_list};
use vc_api::custom_id::ui::select_menu::claim_select;

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

    for (button, (style, emoji)) in buttons.iter().zip([(3, "✅"), (4, "❌"), (1, "🗑️")]) {
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

/// `Listing.selection_select_row/5`'s payload.
fn select_custom_id(ids: &[i64], options: &ListOptions) -> String {
    let mut payload = claim_select().to_vec();
    payload.extend_from_slice(&options.encode());
    payload.extend_from_slice(&vc_api::claim_list::encode_claim_ids(ids));

    vc_api::custom_id::encode(5, &payload)
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
                    "**◻️{}{}**\n状態　: ⌛未決定\n請求額: **{}** `{}`\n請求元: <@{}>\n請求先: <@{}>\n請求日: <t:{}>",
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

    // The title is a bold line, the accent is the container's, and the list comes after the
    // rows: how many rows there are depends on the page, so the list is read from the end.
    let container = &response.body["data"]["components"][0];
    assert_eq!(container["type"], json!(17));
    assert_eq!(container["accent_color"], json!(COLOR_BRAND));
    let children = container["components"].as_array().expect("the children");

    // The title, and then the list; the rows are last, and how many there are depends on the
    // page. So the reading is from the front for what was said and from the back for what was
    // offered.
    assert_eq!(children[0]["content"], json!("**請求一覧(all)**"));
    assert_eq!(
        &children[1..1 + expected.len()],
        expected.as_slice(),
        "the list: {container}"
    );

    let ids: Vec<i64> = page.claims.iter().map(|claim| claim.id).collect();
    let options = list_options(Position::All);

    assert_eq!(
        container["components"][container["components"].as_array().expect("children").len() - 2]["components"],
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

    let select = &container["components"]
        [container["components"].as_array().expect("children").len() - 1]["components"][0];
    assert_eq!(select["type"], json!(3));
    assert_eq!(select["min_values"], json!(0));
    assert_eq!(select["max_values"], json!(ids.len()));
    assert_eq!(select["custom_id"], json!(select_custom_id(&ids, &options)));
    assert_eq!(
        select["options"]
            .as_array()
            .expect("the choices")
            .iter()
            .map(|choice| choice["default"].clone())
            .collect::<Vec<_>>(),
        vec![json!(false); ids.len()]
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

    let mut posted = api.webhooks();
    let body = posted.pop().expect("a follow-up was posted");

    (response, body)
}

async fn assert_action_error(pool: PgPool, action: Action, claim: i64, user: i64, content: &str) {
    let api = fake();
    let (response, body) = press(&api, pool, action, &[claim], user).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
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
    assert_eq!(response.status, 200, "body: {}", response.body);
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
    // The menu is the last row: the page's list comes first, then the rows that move through it.
    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");

    assert_eq!(
        children[children.len() - 1]["components"][0]["max_values"],
        json!(1)
    );

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

    assert_eq!(response.status, 200, "body: {}", response.body);
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

    assert_eq!(response.status, 200, "body: {}", response.body);
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

/// The menu's own payload, which the `:select` render numbers from zero.
fn select_menu_custom_id(k: u8, ids: &[i64]) -> String {
    let mut payload = claim_select().to_vec();
    payload.extend_from_slice(&list_options(Position::All).encode());
    payload.extend_from_slice(&vc_api::claim_list::encode_claim_ids(ids));

    vc_api::custom_id::encode(k, &payload)
}

fn selection_values(ids: &[i64]) -> Value {
    json!(ids.iter().map(|id| id.to_string()).collect::<Vec<_>>())
}

fn ticked(response: &support::Response) -> Vec<bool> {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children")
        .iter()
        // The title is the first Text Display and the quotation is the last: the claims are the
        // ones between them. A claim nobody has ticked carries no mark at all, so filtering on
        // one would drop exactly the claims this is asked about.
        .filter_map(|child| child["content"].as_str())
        .skip(1)
        .take_while(|content| !content.starts_with("**残高"))
        .map(|content| content.starts_with("**☑"))
        .collect()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn selecting_everything_marks_it_and_warns(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let page = first_page(&pool, 1).await;
    let ids: Vec<i64> = page.claims.iter().map(|claim| claim.id).collect();
    assert_eq!(ids.len(), 3);

    let response = interaction(
        router(pool),
        support::select_from_guild(
            json!({
                "custom_id": select_menu_custom_id(0, &ids),
                "values": selection_values(&ids),
            }),
            money.user1,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    // The selection answers by replacing the message the menu was on.
    assert_eq!(response.body["type"], json!(7));
    assert_eq!(response.body["data"]["flags"], json!(32832));

    assert_eq!(ticked(&response), vec![true, true, true]);

    // user1 holds 200000 and is being asked for 10000099, so the quotation warns.
    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");

    assert_eq!(
        children[children.len() - 3]["content"],
        json!(format!(
            "**残高**\n**{}**: `200000{}` - `10000099{}` => `-9800099{}`⚠",
            money.name, money.unit, money.unit, money.unit
        ))
    );

    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");

    // The menu is the first row, and the rows are last: the quotation is between them.
    let menu = &children[children.len() - 2]["components"][0];
    assert_eq!(menu["type"], json!(3));
    assert_eq!(menu["max_values"], json!(3));
    assert_eq!(menu["custom_id"], json!(select_menu_custom_id(0, &ids)));

    // user1 is not the payer of all three, so nothing can be acted on.
    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");
    let buttons = children[children.len() - 1]["components"]
        .as_array()
        .expect("the buttons");
    assert_eq!(buttons.len(), 4);
    assert!(
        buttons[0]["disabled"].is_null(),
        "going back is always possible"
    );
    for button in &buttons[1..] {
        assert_eq!(button["disabled"], json!(true));
    }
}

/// c6 is user1's claim on themselves for 100, which they can afford and are both
/// sides of, so every action is offered.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn selecting_one_affordable_claim_enables_the_actions(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;
    let selected = claims.id(5);

    let page = first_page(&pool, 1).await;
    let ids: Vec<i64> = page.claims.iter().map(|claim| claim.id).collect();

    let response = interaction(
        router(pool),
        support::select_from_guild(
            json!({
                "custom_id": select_menu_custom_id(0, &ids),
                "values": selection_values(&[selected]),
            }),
            money.user1,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");

    assert_eq!(
        children[children.len() - 3]["content"],
        json!(format!(
            "**残高**\n**{}**: `200000{}` - `100{}` => `199900{}`",
            money.name, money.unit, money.unit, money.unit
        ))
    );

    // The page is newest first, and c6 is the newest of the three.
    assert_eq!(ticked(&response), vec![true, false, false]);

    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the children");
    let buttons = children[children.len() - 1]["components"]
        .as_array()
        .expect("the buttons");
    for button in &buttons[1..] {
        assert_eq!(button["disabled"], json!(false));
    }
}

/// user2 is a party to two of the three, but not all three, and the menu only
/// ever offered their own.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn selecting_someone_elses_claims_is_refused(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let page = first_page(&pool, 1).await;
    let ids: Vec<i64> = page.claims.iter().map(|claim| claim.id).collect();

    let response = interaction(
        router(pool),
        support::select_from_guild(
            json!({
                "custom_id": select_menu_custom_id(0, &ids),
                "values": selection_values(&ids),
            }),
            money.user2,
        ),
    )
    .await;

    // Elixir raises `ArgumentError, "Illegal request"`, which is a 500.
    assert_eq!(response.status, 500, "body: {}", response.body);
}
