//! The `claim make` and `claim show` commands, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/claim/claim_make_test.exs`
//! and `claim_show_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{ClaimSet, execute_from_guild, fake, get_amount, interaction, setup_claim, state};

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
    assert_eq!(body["data"]["flags"], json!(64));
    assert_eq!(body["data"]["content"], json!(""));

    let embed = &body["data"]["embeds"][0];
    assert_eq!(embed["title"], json!("請求"));
    assert_eq!(embed["color"], json!(COLOR_BRAND));
    assert_eq!(embed["fields"][0]["name"], json!(format!("📤📥{claim_id}")));
    assert_eq!(
        embed["fields"][0]["value"],
        json!(format!(
            "状態　: ⌛未決定\n請求額: **100** `{}`\n請求元: <@{}>\n請求先: <@{}>\n請求日: <t:{inserted_at}>",
            money.unit, money.user1, money.user1
        ))
    );

    let quotation = &body["data"]["embeds"][1];
    assert_eq!(quotation["title"], json!("残高"));
    assert_eq!(quotation["color"], json!(COLOR_BRAND));
    assert_eq!(
        quotation["description"],
        json!(format!(
            "**{}**: `200000{}` - `100{}` => `199900{}`",
            money.name, money.unit, money.unit, money.unit
        ))
    );

    let row = &body["data"]["components"][0];
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

    let description = response.body["data"]["embeds"][0]["description"]
        .as_str()
        .expect("a description")
        .to_string();
    assert_eq!(response.body["data"]["flags"], json!(64));
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
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "title": "エラー",
                    "color": COLOR_ERROR,
                    "description": "不正な金額です。1以上9223372036854775807以下である必要があります。",
                }],
            },
        })
    );
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
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "title": "エラー",
                    "color": COLOR_ERROR,
                    "description": "指定された通貨は存在しません。",
                }],
            },
        })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn show_reports_an_unknown_claim(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(router(pool), show_from_guild(money.user1, -1)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "title": "エラー",
                    "color": COLOR_ERROR,
                    "description": "そのidの請求は見つかりませんでした。",
                }],
            },
        })
    );
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
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "title": "エラー",
                    "color": COLOR_ERROR,
                    "description": "そのidの請求は見つかりませんでした。",
                }],
            },
        })
    );
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
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "title": "エラー",
                    "color": COLOR_ERROR,
                    "description": message,
                }],
            },
        })
    );
}

/// `Interactions.Claim.render/1` for a transition the caller was allowed to make.
fn assert_action_result(response: &support::Response, claim_id: i64, result: &str) {
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "description": format!("id: {claim_id}の請求を{result}"),
                    "color": COLOR_OK,
                }],
            },
        })
    );
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
