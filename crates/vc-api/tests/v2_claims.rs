//! Contract tests for `GET /api/v2/users/@me/claims/:id`, ported from the Elixir
//! suite (`test/virtualCrypto_web/controllers/api/v2/claim/claim_controller_test.exs`).

mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    CLAIM_AT_RFC3339, Response, assert_json, fake, get, insert_asset, insert_claim,
    insert_claim_metadata, insert_currency, insert_user, mint, state,
};

const GUILD: i64 = 900_000_000_000_000_001;
const CURRENCY_ID: i64 = 1;
const CLAIM_ID: i64 = 1;
const USER1: i32 = 1;
const USER2: i32 = 2;
const USER3: i32 = 3;
const DISCORD1: i64 = 100_000_000_000_000_001;
const DISCORD2: i64 = 100_000_000_000_000_002;
const DISCORD3: i64 = 100_000_000_000_000_003;

fn claim_json(
    amount: &str,
    claimant: i32,
    claimant_discord: i64,
    payer: i32,
    payer_discord: i64,
    status: &str,
    metadata: Value,
) -> Value {
    json!({
        "id": CLAIM_ID.to_string(),
        "currency": {
            "name": "nyan",
            "unit": "nyan",
            "guild": GUILD.to_string(),
            "pool_amount": "500",
        },
        "amount": amount,
        "claimant": {
            "id": claimant.to_string(),
            "discord": { "id": claimant_discord.to_string() },
        },
        "payer": {
            "id": payer.to_string(),
            "discord": { "id": payer_discord.to_string() },
        },
        "created_at": CLAIM_AT_RFC3339,
        "updated_at": CLAIM_AT_RFC3339,
        "status": status,
        "metadata": metadata,
    })
}

fn pending_claim(metadata: Value) -> Value {
    claim_json("500", USER1, DISCORD1, USER2, DISCORD2, "pending", metadata)
}

async fn fixture(pool: &PgPool) {
    insert_user(pool, USER1, DISCORD1).await;
    insert_user(pool, USER2, DISCORD2).await;
    insert_user(pool, USER3, DISCORD3).await;
    insert_currency(pool, CURRENCY_ID, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, USER1, CURRENCY_ID, 199_500).await;
    insert_asset(pool, USER2, CURRENCY_ID, 1_000).await;
    insert_claim(pool, CLAIM_ID, 500, "pending", USER1, USER2, CURRENCY_ID).await;
}

async fn request(pool: PgPool, id: &str, token: Option<&str>) -> Response {
    get(
        vc_api::router(state(pool, fake())),
        &format!("/api/v2/users/@me/claims/{id}"),
        token,
    )
    .await
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_claimant_can_read_the_claim(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = request(pool, &CLAIM_ID.to_string(), Some(&token)).await;

    assert_json(&response, 200, pending_claim(json!({})));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_payer_can_read_the_claim(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER2, &["vc.claim"]).await;

    let response = request(pool, &CLAIM_ID.to_string(), Some(&token)).await;

    assert_json(&response, 200, pending_claim(json!({})));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unrelated_user_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER3, &["vc.claim"]).await;

    let response = request(pool, &CLAIM_ID.to_string(), Some(&token)).await;

    assert_json(
        &response,
        403,
        json!({ "error": "forbidden", "error_description": "not_related_user" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_without_the_claim_scope_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["oauth2.register"]).await;

    let response = request(pool, &CLAIM_ID.to_string(), Some(&token)).await;

    assert_json(
        &response,
        403,
        json!({ "error": "invalid_token", "error_description": "permission_denied" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_claim_is_not_found(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = request(pool, "123", Some(&token)).await;

    assert_json(
        &response,
        404,
        json!({ "error": "not_found", "error_description": "not_found" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_numeric_claim_id_is_not_found(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = request(pool, "abc", Some(&token)).await;

    assert_json(
        &response,
        404,
        json!({ "error": "not_found", "error_description": "not_found" }),
    );
}

/// The metadata join is restricted to the requesting user, so each side only
/// ever sees its own metadata.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn metadata_is_scoped_to_the_requesting_user(pool: PgPool) {
    fixture(&pool).await;
    insert_claim_metadata(
        &pool,
        CLAIM_ID,
        USER1,
        USER2,
        USER2,
        json!({ "note": "from the payer" }),
    )
    .await;

    let claimant_token = mint(&pool, USER1, &["vc.claim"]).await;
    let response = request(pool.clone(), &CLAIM_ID.to_string(), Some(&claimant_token)).await;
    assert_json(&response, 200, pending_claim(json!({})));

    let payer_token = mint(&pool, USER2, &["vc.claim"]).await;
    let response = request(pool, &CLAIM_ID.to_string(), Some(&payer_token)).await;
    assert_json(
        &response,
        200,
        pending_claim(json!({ "note": "from the payer" })),
    );
}
