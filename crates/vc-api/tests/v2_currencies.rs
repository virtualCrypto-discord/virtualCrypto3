//! Contract tests for `GET /api/v2/currencies` and
//! `GET /api/v2/currencies/:id`, ported from the Elixir suite
//! (`test/virtualCrypto_web/controllers/api/v2/currencies_controller_test.exs`).
//!
//! The Elixir suite builds its fixture with `setup_money/1`. Here the same state
//! is inserted directly: one currency whose asset rows sum to 200500, with a pool
//! of 500.

mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    Response, assert_json, fake, get, insert_asset, insert_currency, insert_user, state,
};

const GUILD: i64 = 900_000_000_000_000_001;
const CURRENCY_ID: i64 = 1;

fn success() -> Value {
    json!({
        "total_amount": "200500",
        "name": "nyan",
        "unit": "nyan",
        "guild": GUILD.to_string(),
        "pool_amount": "500",
    })
}

fn not_found() -> Value {
    json!({ "error": "not_found", "error_description": "not_found" })
}

fn need_one_parameter() -> Value {
    json!({
        "error": "invalid_request",
        "error_description": "need_one_parameter_from_id_guild_name_or_unit",
    })
}

fn invalid_id() -> Value {
    json!({
        "error": "invalid_request",
        "error_description": "id_must_be_positive_integer",
    })
}

fn invalid_guild_id() -> Value {
    json!({
        "error": "invalid_request",
        "error_description": "guild_id_must_be_positive_integer",
    })
}

async fn fixture(pool: &PgPool) {
    insert_user(pool, 1, 100_000_000_000_000_001).await;
    insert_user(pool, 2, 100_000_000_000_000_002).await;
    insert_currency(pool, CURRENCY_ID, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, 1, CURRENCY_ID, 199_500).await;
    insert_asset(pool, 2, CURRENCY_ID, 1_000).await;
}

async fn request(pool: PgPool, path: &str) -> Response {
    get(vc_api::router(state(pool, fake())), path, None).await
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn not_supplying_a_parameter_is_400(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies").await;

    assert_json(&response, 400, need_one_parameter());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn supplying_an_unknown_named_parameter_is_400(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?invalid=x").await;

    assert_json(&response, 400, need_one_parameter());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn supplying_too_many_parameters_is_400(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?guild=1&id=1").await;

    assert_json(&response, 400, need_one_parameter());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_path_id_and_a_query_parameter_together_is_400(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies/1?guild=1").await;

    assert_json(&response, 400, need_one_parameter());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_numeric_guild_is_400(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?guild=x").await;

    assert_json(&response, 400, invalid_guild_id());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_negative_guild_is_400(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?guild=-21").await;

    assert_json(&response, 400, invalid_guild_id());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_guild_is_404(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?guild=123").await;

    assert_json(&response, 404, not_found());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_matching_guild_is_200(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, &format!("/api/v2/currencies?guild={GUILD}")).await;

    assert_json(&response, 200, success());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_unit_is_404(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?unit=x").await;

    assert_json(&response, 404, not_found());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_matching_unit_is_200(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?unit=nyan").await;

    assert_json(&response, 200, success());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_name_is_404(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?name=wan").await;

    assert_json(&response, 404, not_found());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_matching_name_is_200(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?name=nyan").await;

    assert_json(&response, 200, success());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_numeric_id_is_400(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?id=x").await;

    assert_json(&response, 400, invalid_id());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_negative_id_is_400(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?id=-21").await;

    assert_json(&response, 400, invalid_id());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_id_is_404(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, "/api/v2/currencies?id=123").await;

    assert_json(&response, 404, not_found());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_matching_id_is_200(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, &format!("/api/v2/currencies?id={CURRENCY_ID}")).await;

    assert_json(&response, 200, success());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_matching_id_in_the_path_is_200(pool: PgPool) {
    fixture(&pool).await;

    let response = request(pool, &format!("/api/v2/currencies/{CURRENCY_ID}")).await;

    assert_json(&response, 200, success());
}

/// `Money.info/1` joins `assets`, so a currency with no assets has no row and is
/// reported as not found even though the currency exists.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_currency_without_assets_is_404(pool: PgPool) {
    fixture(&pool).await;
    insert_currency(&pool, 2, "wan", "wan", GUILD + 1, 1000).await;

    let response = request(pool, "/api/v2/currencies?name=wan").await;

    assert_json(&response, 404, not_found());
}
