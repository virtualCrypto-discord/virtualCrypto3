//! The `create` command, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/create_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    DEFAULT_PERMISSIONS, Response, currency_by_unit, execute_from_guild, fake, get_amount,
    interaction, setup_money, state,
};

const COLOR_OK: i64 = 0x0038_EA42;
const COLOR_ERROR: i64 = 0x00EA_3875;
/// A guild with no currency, which is neither the interaction default nor either
/// `setup_money` guild.
const FREE_GUILD: i64 = 494_780_225_280_802_818;

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// `InteractionsControllerTest.Create.Helper.create_data/1`.
fn create_data(amount: Value, unit: &str, name: &str) -> Value {
    json!({
        "name": "create",
        "options": [
            { "name": "amount", "value": amount },
            { "name": "unit", "value": unit },
            { "name": "name", "value": name },
        ],
    })
}

fn from_guild(amount: Value, unit: &str, name: &str, sender: i64) -> Value {
    execute_from_guild(create_data(amount, unit, name), sender)
}

fn from_guild_in(
    amount: Value,
    unit: &str,
    name: &str,
    sender: i64,
    guild_id: i64,
    permissions: &str,
) -> Value {
    support::from_guild(
        create_data(amount, unit, name),
        sender,
        guild_id,
        permissions,
    )
}

/// `Interactions.Create.render/3` for `{:ok, :ok, options}`.
fn assert_ok(response: &Response, unit: &str) {
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": COLOR_OK,
            "components": [{
                "type": 10,
                "content": format!(
                    "✅ 通貨の作成に成功しました！ `/info unit: {unit}`コマンドで通貨の情報をご覧ください。\n\
                     削除したい場合は、72時間以内に`/delete`コマンドを実行してください。"
                ),
            }],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
}

/// `Interactions.Create.render/3` for `{:error, reason, options}`.
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
async fn create_makes_the_currency_and_the_creators_grant(pool: PgPool) {
    setup_money(&pool).await;
    let sender = 100_000_000_000_000_101;

    let response = interaction(
        router(pool.clone()),
        from_guild(json!(10000), "ua", "funyu1", sender),
    )
    .await;

    assert_ok(&response, "ua");

    let currency = currency_by_unit(&pool, "ua").await.expect("the currency");
    assert_eq!(currency.name.as_deref(), Some("funyu1"));
    // The pool is a two-hundredth of the grant, rounded up.
    assert_eq!(currency.pool_amount, Some((10000 + 199) / 200));
    assert_eq!(get_amount(&pool, sender, currency.id).await, 10000);
}

/// The message it answers with names two commands, and a name is a link when Discord's ids
/// are known: the same rule the document's prose follows, because the same person reads both.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_created_message_links_the_commands_it_names(pool: PgPool) {
    setup_money(&pool).await;
    let sender = 100_000_000_000_000_101;

    let response = interaction(
        vc_api::router(state(
            pool,
            support::FakeDiscord::with_commands(&[("info", 7), ("delete", 8)]),
        )),
        from_guild(json!(10000), "ub", "funyu2", sender),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(
        rendered.contains("</info:7> `unit: ub`コマンドで通貨の情報をご覧ください。"),
        "{rendered}"
    );
    assert!(
        rendered.contains("72時間以内に</delete:8>コマンドを実行してください。"),
        "{rendered}"
    );
}

/// Elixir calls this case "not admin without v2 flag" and passes the default
/// permissions, which are every bit set — so what it actually pins is that a
/// guild other than the seeded ones works. The flag it was named for is the
/// branch this port dropped, because every guild carries it now.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_succeeds_in_another_guild(pool: PgPool) {
    setup_money(&pool).await;
    let sender = 100_000_000_000_000_102;

    let response = interaction(
        router(pool.clone()),
        from_guild_in(
            json!(10000),
            "ub",
            "funyu2",
            sender,
            FREE_GUILD,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_ok(&response, "ub");

    let currency = currency_by_unit(&pool, "ub").await.expect("the currency");
    assert_eq!(currency.pool_amount, Some((10000 + 199) / 200));
    assert_eq!(get_amount(&pool, sender, currency.id).await, 10000);
}

/// Names are checked across guilds, so the name of another guild's currency is
/// already taken.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_rejects_a_currency_name_that_is_taken(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild_in(
            json!(10000),
            "uc",
            &money.name,
            100_000_000_000_000_103,
            FREE_GUILD,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_error(
        &response,
        &format!(
            "`{}`という名前の通貨は存在しています。別の名前を使用してください。",
            money.name
        ),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_rejects_a_currency_unit_that_is_taken(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild_in(
            json!(10000),
            &money.unit,
            "funyu4",
            100_000_000_000_000_104,
            FREE_GUILD,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_error(
        &response,
        &format!(
            "`{}`という単位の通貨は存在しています。別の単位を使用してください。",
            money.unit
        ),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_rejects_a_grant_above_the_limit(pool: PgPool) {
    setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild_in(
            json!("9007199254740992"),
            "ue",
            "funyu5",
            100_000_000_000_000_105,
            FREE_GUILD,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_error(
        &response,
        "不正な金額です。1以上4294967295以下である必要があります。",
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_needs_the_administrator_bit(pool: PgPool) {
    setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild_in(
            json!(10000),
            "uf",
            "funyu6",
            100_000_000_000_000_106,
            1_234_567_890_123_456_789,
            "0",
        ),
    )
    .await;

    assert_error(&response, "実行には管理者権限が必要です。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_rejects_a_guild_that_already_has_a_currency(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild_in(
            json!(10000),
            "ug",
            "funyu7",
            100_000_000_000_000_107,
            money.guild,
            DEFAULT_PERMISSIONS,
        ),
    )
    .await;

    assert_error(&response, "このギルドではすでに通貨が作成されています。");
}

/// The unit's letters and length are the sentence's too. The regexes took any
/// lowercase letter anywhere in it, so `u1` was a unit and so was one of eleven
/// characters.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_rejects_a_unit_that_is_not_one_to_ten_lowercase_letters(pool: PgPool) {
    setup_money(&pool).await;

    for unit in ["AA", "u1", "abcdefghijk"] {
        let response = interaction(
            router(pool.clone()),
            from_guild(json!(10000), unit, "funyu8", 100_000_000_000_000_108),
        )
        .await;

        assert_error(
            &response,
            "通貨の名前は2から16文字以内の英数字、単位は1から10文字以内の英小文字を使ってください。",
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_rejects_a_name_without_enough_alphanumerics(pool: PgPool) {
    setup_money(&pool).await;

    let response = interaction(
        router(pool),
        from_guild(json!(10000), "a", " ", 100_000_000_000_000_109),
    )
    .await;

    assert_error(
        &response,
        "通貨の名前は2から16文字以内の英数字、単位は1から10文字以内の英小文字を使ってください。",
    );
}

/// The name's length is the sentence's, not the regexes' — they matched anywhere
/// in the string, so a name of any length and any punctuation around it went
/// through.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn create_rejects_a_name_that_is_too_long_or_has_a_mark_in_it(pool: PgPool) {
    setup_money(&pool).await;

    for (unit, name) in [("ua", "0123456789abcdefg"), ("ub", "funyua!")] {
        let response = interaction(
            router(pool.clone()),
            from_guild(json!(10000), unit, name, 100_000_000_000_000_110),
        )
        .await;

        assert_error(
            &response,
            "通貨の名前は2から16文字以内の英数字、単位は1から10文字以内の英小文字を使ってください。",
        );
    }
}
