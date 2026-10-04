//! The `info` command, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/info_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    DEFAULT_PERMISSIONS, Response, execute_from_dm, fake, fake_with_guild, interaction,
    setup_money, state,
};
use vc_api::command::amount_text;

const COLOR_BRAND: i64 = 0x0062_21ED;
const COLOR_ERROR: i64 = 0x00EA_3875;
const FOOTER: &str = "発行枠は1日1回、総発行量の0.5%ずつ増えます（上限は総発行量の3.5%）。";
/// setup_money leaves 199500 with the first user and 1000 with the second.
const TOTAL: i64 = 200_500;
const POOL: i64 = 500;

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// A router whose Discord reports `guild`, for the icon branches.
fn router_with_guild(pool: PgPool, guild: Value) -> Router {
    vc_api::router(state(pool, fake_with_guild(guild)))
}

/// `InteractionsControllerTest.Info.Helper`.
fn info_data(options: Vec<Value>) -> Value {
    json!({ "name": "info", "options": options })
}

fn from_guild(sender: i64, guild_id: i64) -> Value {
    support::from_guild(info_data(Vec::new()), sender, guild_id, DEFAULT_PERMISSIONS)
}

fn from_guild_name(name: &str, sender: i64, guild_id: i64) -> Value {
    support::from_guild(
        info_data(vec![json!({ "name": "name", "value": name })]),
        sender,
        guild_id,
        DEFAULT_PERMISSIONS,
    )
}

fn from_guild_unit(unit: &str, sender: i64, guild_id: i64) -> Value {
    support::from_guild(
        info_data(vec![json!({ "name": "unit", "value": unit })]),
        sender,
        guild_id,
        DEFAULT_PERMISSIONS,
    )
}

fn from_dm(sender: i64) -> Value {
    execute_from_dm(info_data(Vec::new()), sender)
}

/// `Interactions.Info.render/2` for `:ok`, as the components it is now.
///
/// TestGuild has no icon in these, so its field is a Text Display rather than a section: a section
/// requires an accessory, and no icon is none to give it.
fn embed(name: &str, unit: &str, amount: i64) -> Value {
    json!([{
        "type": 17,
        "accent_color": COLOR_BRAND,
        "components": [
            { "type": 10, "content": format!("**通貨名: {name}**") },
            { "type": 10, "content": "サーバー名: TestGuild" },
            { "type": 10, "content": format!(
                "単位: `{unit}`\n総発行量: **{}** `{unit}`\n発行枠: **{}** `{unit}`\nあなたの残高: **{}** `{unit}`\n削除可能: はい",
                amount_text(TOTAL),
                amount_text(POOL),
                amount_text(amount),
            ) },
            { "type": 10, "content": format!("-# {FOOTER}") },
        ],
    }])
}

/// The guild's field, which is a section when there is an icon to put beside it and a Text Display
/// of its own when there is not: a section requires an accessory, so no icon means no section.
fn guild_line(body: &Value) -> String {
    let line = &body["data"]["components"][0]["components"][1];

    let text = if line["type"] == 9 {
        &line["components"][0]["content"]
    } else {
        &line["content"]
    };

    text.as_str().unwrap_or_default().to_owned()
}

fn assert_ok(response: &Response, embed: Value) {
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["data"]["components"], embed);
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

/// `Interactions.Info.render/2` for `:error`, which both error cases share.
fn assert_error(response: &Response, description: &str) {
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_ERROR,
            "components": [{
                "type": 10,
                "content": format!("**エラー**\n{description}"),
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_in_guild(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild(money.user1, money.guild)).await;

    assert_ok(&response, embed(&money.name, &money.unit, 199_500));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_in_guild_without_holding_the_currency(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild(-1, money.guild)).await;

    assert_ok(&response, embed(&money.name, &money.unit, 0));
}

/// A guild with no currency of its own, and no other selector to fall back on.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_in_a_guild_without_a_currency_is_not_found(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild(money.user1, -1)).await;

    assert_error(&response, "通貨が見つかりません。");
}

/// A supplied unit is used instead of the guild, so the guild id does not have
/// to match the currency's.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_by_unit(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild_unit(&money.unit, money.user1, -1)).await;

    assert_ok(&response, embed(&money.name, &money.unit, 199_500));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_by_an_unknown_unit_is_not_found(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild_unit("gao", money.user1, -1)).await;

    assert_error(&response, "通貨が見つかりません。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_by_unit_without_holding_the_currency(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild_unit(&money.unit, -21, -1)).await;

    assert_ok(&response, embed(&money.name, &money.unit, 0));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_by_name(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild_name(&money.name, money.user1, -1)).await;

    assert_ok(&response, embed(&money.name, &money.unit, 199_500));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_by_an_unknown_name_is_not_found(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild_name("fuwafuwa", money.user1, -1)).await;

    assert_error(&response, "通貨が見つかりません。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_by_name_without_holding_the_currency(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_guild_name(&money.name, -1, -1)).await;

    assert_ok(&response, embed(&money.name, &money.unit, 0));
}

/// A direct message carries no guild, so with neither name nor unit there is
/// nothing to look the currency up by.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn info_run_in_a_direct_message(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), from_dm(money.user1)).await;

    assert_error(&response, "DMでは `name` か `unit` を指定してください。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_without_an_icon_has_no_icon_url(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router_with_guild(pool, json!({ "name": "TestGuild" })),
        from_guild_unit(&money.unit, money.user1, -1),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        guild_line(&response.body),
        "サーバー名: TestGuild",
        "{}",
        response.body
    );
    assert!(
        response.body["data"]["components"][0]["components"][1]
            .get("accessory")
            .is_none(),
        "no icon is no thumbnail: {}",
        response.body
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_icon_url_is_webp(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router_with_guild(
            pool,
            json!({
                "name": "TestGuild",
                "icon": "981b65442cb7cffa5a60b6b94a10d263",
            }),
        ),
        from_guild_unit(&money.unit, money.user1, -1),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"][0]["components"][1]["accessory"]["media"]["url"],
        json!(format!(
            "https://cdn.discordapp.com/icons/{}/981b65442cb7cffa5a60b6b94a10d263.webp",
            money.guild
        )),
        "{}",
        response.body
    );
    assert_eq!(
        guild_line(&response.body),
        "サーバー名: TestGuild",
        "{}",
        response.body
    );
}

/// An `a_` prefix marks the icon as animated, so it is served as a gif.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_animated_guild_icon_url_is_gif(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router_with_guild(
            pool,
            json!({
                "name": "TestGuild",
                "icon": "a_981b65442cb7cffa5a60b6b94a10d263",
            }),
        ),
        from_guild_unit(&money.unit, money.user1, -1),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"][0]["components"][1]["accessory"]["media"]["url"],
        json!(format!(
            "https://cdn.discordapp.com/icons/{}/a_981b65442cb7cffa5a60b6b94a10d263.gif",
            money.guild
        )),
        "{}",
        response.body
    );
    assert_eq!(
        guild_line(&response.body),
        "サーバー名: TestGuild",
        "{}",
        response.body
    );
}
