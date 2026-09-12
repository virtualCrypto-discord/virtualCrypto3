//! Contract tests for `POST /api/v2/users/@me/claims`, ported from
//! `ClaimControllerTest.V2` in
//! `test/virtualCrypto_web/controllers/api/v2/claim/claim_controller_test.exs`
//! and the creation case in `.../claim/metadata/update_test.exs`.

mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{Response, assert_json, fake, insert_currency, insert_user, mint, state};
use tower::ServiceExt;

const GUILD: i64 = 900_000_000_000_000_001;
const CURRENCY_ID: i64 = 1;
const USER1: i32 = 1;
const USER2: i32 = 2;
const DISCORD1: i64 = 100_000_000_000_000_001;
const DISCORD2: i64 = 100_000_000_000_000_002;
/// A discord user that has no account yet, so creating a claim against them has
/// to create one.
const UNKNOWN_DISCORD: i64 = 100_000_000_000_000_009;

async fn fixture(pool: &PgPool) {
    insert_user(pool, USER1, DISCORD1).await;
    insert_user(pool, USER2, DISCORD2).await;
    insert_currency(pool, CURRENCY_ID, "nyan", "nyan", GUILD, 500).await;
}

async fn post(pool: PgPool, token: &str, body: Value) -> Response {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/v2/users/@me/claims")
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("encode body"),
        ))
        .expect("request");

    let response = vc_api::router(state(pool, fake()))
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };

    Response {
        status,
        headers,
        body,
    }
}

async fn claim_token(pool: &PgPool) -> String {
    mint(pool, USER1, &["vc.claim"]).await
}

/// Everything except the timestamps, which move.
fn created_claim(amount: &str, payer: i32, metadata: Value) -> Value {
    let discord = |user: i32| match user {
        USER1 => DISCORD1,
        _ => DISCORD2,
    };

    json!({
        "id": "1",
        "currency": {
            "name": "nyan",
            "unit": "nyan",
            "guild": GUILD.to_string(),
            "pool_amount": "500",
        },
        "amount": amount,
        "claimant": {
            "id": USER1.to_string(),
            "discord": { "id": discord(USER1).to_string() },
        },
        "payer": {
            "id": payer.to_string(),
            "discord": { "id": discord(payer).to_string() },
        },
        "created_at": Value::Null,
        "updated_at": Value::Null,
        "status": "pending",
        "metadata": metadata,
    })
}

fn assert_created(response: &Response, mut expected: Value) {
    expected["created_at"] = response.body["created_at"].clone();
    expected["updated_at"] = response.body["updated_at"].clone();

    assert_json(response, 201, expected);
}

// --- rejected bodies -------------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_empty_body_is_missing_the_payer(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(pool, &token, json!({})).await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "payer_discord_id_field_is_required" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payer_without_an_amount_is_missing_the_amount(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(
        pool,
        &token,
        json!({ "payer_discord_id": DISCORD2.to_string(), "unit": "nyan" }),
    )
    .await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "amount_field_is_required" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payer_without_a_unit_is_missing_the_unit(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(
        pool,
        &token,
        json!({ "payer_discord_id": DISCORD2.to_string() }),
    )
    .await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "unit_field_is_required" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_string_payer_is_rejected_before_a_non_string_amount(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(
        pool,
        &token,
        json!({ "payer_discord_id": DISCORD2, "unit": "nyan", "amount": 20 }),
    )
    .await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "invalid_payer_discord_id_type" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_string_amount_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(
        pool,
        &token,
        json!({ "payer_discord_id": DISCORD2.to_string(), "unit": "nyan", "amount": 20 }),
    )
    .await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "invalid_amount_type" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unparseable_payer_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    for payer in ["nyan", "2nyan"] {
        let response = post(
            pool.clone(),
            &token,
            json!({ "payer_discord_id": payer, "unit": "nyan", "amount": "20" }),
        )
        .await;

        assert_json(
            &response,
            400,
            json!({ "error": "invalid_request", "error_description": "invalid_payer_discord_id_value" }),
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unparseable_amount_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    for amount in ["nyan", "2nyan"] {
        let response = post(
            pool.clone(),
            &token,
            json!({ "payer_discord_id": DISCORD2.to_string(), "unit": "nyan", "amount": amount }),
        )
        .await;

        assert_json(
            &response,
            400,
            json!({ "error": "invalid_request", "error_description": "invalid_amount_value" }),
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_positive_amount_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    for amount in ["0", "-5"] {
        let response = post(
            pool.clone(),
            &token,
            json!({ "payer_discord_id": DISCORD2.to_string(), "unit": "nyan", "amount": amount }),
        )
        .await;

        assert_json(
            &response,
            400,
            json!({ "error": "invalid_request", "error_description": "invalid_amount" }),
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_unit_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(
        pool,
        &token,
        json!({ "payer_discord_id": DISCORD2.to_string(), "unit": "wan", "amount": "20" }),
    )
    .await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "not_found_currency" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_without_the_claim_scope_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = post(
        pool,
        &token,
        json!({ "payer_discord_id": DISCORD2.to_string(), "unit": "nyan", "amount": "20" }),
    )
    .await;

    assert_json(
        &response,
        403,
        json!({ "error": "invalid_token", "error_description": "permission_denied" }),
    );
}

// --- creation --------------------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_claim_is_created_for_a_known_payer(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(
        pool,
        &token,
        json!({ "payer_discord_id": DISCORD2.to_string(), "unit": "nyan", "amount": "20" }),
    )
    .await;

    assert_created(&response, created_claim("20", USER2, json!({})));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn metadata_can_be_set_when_creating(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(
        pool,
        &token,
        json!({
            "payer_discord_id": DISCORD2.to_string(),
            "unit": "nyan",
            "amount": "20",
            "metadata": { "a": "b" },
        }),
    )
    .await;

    assert_created(&response, created_claim("20", USER2, json!({ "a": "b" })));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_payer_gains_an_account(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(
        pool.clone(),
        &token,
        json!({ "payer_discord_id": UNKNOWN_DISCORD.to_string(), "unit": "nyan", "amount": "20" }),
    )
    .await;

    assert_eq!(response.status, 201, "the claim is created");

    let created = sqlx::query!(
        "SELECT id FROM users WHERE discord_id = $1",
        UNKNOWN_DISCORD
    )
    .fetch_optional(&pool)
    .await
    .expect("read user")
    .is_some();

    assert!(created, "resolving the payer creates their account");
}

/// A claim's metadata belongs to the claimant, so the payer cannot see it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn created_metadata_is_owned_by_the_claimant(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool).await;

    let response = post(
        pool.clone(),
        &token,
        json!({
            "payer_discord_id": DISCORD2.to_string(),
            "unit": "nyan",
            "amount": "20",
            "metadata": { "note": "mine" },
        }),
    )
    .await;
    assert_eq!(response.status, 201);

    let row = sqlx::query!(
        "SELECT owner_user_id FROM claim_metadata WHERE claim_id = $1",
        1i64
    )
    .fetch_one(&pool)
    .await
    .expect("read metadata row");

    assert_eq!(row.owner_user_id, i64::from(USER1), "owned by the claimant");
}
