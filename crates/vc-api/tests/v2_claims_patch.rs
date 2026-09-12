//! Contract tests for `PATCH /api/v2/users/@me/claims/:id`, ported from
//! `ClaimControllerTest.V2` in
//! `test/virtualCrypto_web/controllers/api/v2/claim/claim_controller_test.exs`.

mod support;

use std::sync::Arc;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    FakeDiscord, Response, assert_json, fake, insert_asset, insert_claim, insert_currency,
    insert_user, mint, state,
};
use tower::ServiceExt;

const GUILD: i64 = 900_000_000_000_000_001;
const CURRENCY_ID: i64 = 1;
const USER1: i32 = 1;
const USER2: i32 = 2;
const UNRELATED: i32 = 3;
const DISCORD1: i64 = 100_000_000_000_000_001;
const DISCORD2: i64 = 100_000_000_000_000_002;
const DISCORD3: i64 = 100_000_000_000_000_003;

/// claim 1: user1 -> user2, 500, pending
/// claim 2: user2 -> user1, 9999999, pending (user1 cannot cover it)
/// claim 3: user1 -> user2, 500, already approved
const CLAIM1: i64 = 1;
const CLAIM2: i64 = 2;
const CLAIM3: i64 = 3;

async fn fixture(pool: &PgPool) {
    insert_user(pool, USER1, DISCORD1).await;
    insert_user(pool, USER2, DISCORD2).await;
    insert_user(pool, UNRELATED, DISCORD3).await;
    insert_currency(pool, CURRENCY_ID, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, USER1, CURRENCY_ID, 199_500).await;
    insert_asset(pool, USER2, CURRENCY_ID, 1_000).await;

    insert_claim(pool, CLAIM1, 500, "pending", USER1, USER2, CURRENCY_ID).await;
    insert_claim(
        pool,
        CLAIM2,
        9_999_999,
        "pending",
        USER2,
        USER1,
        CURRENCY_ID,
    )
    .await;
    insert_claim(pool, CLAIM3, 500, "approved", USER1, USER2, CURRENCY_ID).await;
}

async fn amount(pool: &PgPool, user: i32, currency: i64) -> Option<i64> {
    sqlx::query!(
        "SELECT amount FROM assets WHERE user_id = $1 AND currency_id = $2",
        i64::from(user),
        currency
    )
    .fetch_optional(pool)
    .await
    .expect("read asset")
    .and_then(|row| row.amount)
}

async fn patch(app: Router, id: i64, token: &str, body: Value) -> Response {
    let request = axum::http::Request::builder()
        .method("PATCH")
        .uri(format!("/api/v2/users/@me/claims/{id}"))
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("encode body"),
        ))
        .expect("request");

    let response = app.oneshot(request).await.expect("router response");
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

/// The response's currency, claimant and payer are fixed by the fixture; only the
/// timestamps move, so they are copied from the response before comparing.
fn claim_json(id: i64, amount: &str, claimant: i32, payer: i32, status: &str) -> Value {
    let discord = |user: i32| match user {
        USER1 => DISCORD1,
        USER2 => DISCORD2,
        _ => DISCORD3,
    };

    json!({
        "id": id.to_string(),
        "currency": {
            "name": "nyan",
            "unit": "nyan",
            "guild": GUILD.to_string(),
            "pool_amount": "500",
        },
        "amount": amount,
        "claimant": {
            "id": claimant.to_string(),
            "discord": { "id": discord(claimant).to_string() },
        },
        "payer": {
            "id": payer.to_string(),
            "discord": { "id": discord(payer).to_string() },
        },
        "created_at": Value::Null,
        "updated_at": Value::Null,
        "status": status,
        "metadata": {},
    })
}

fn assert_claim(response: &Response, status: u16, mut expected: Value) {
    expected["created_at"] = response.body["created_at"].clone();
    expected["updated_at"] = response.body["updated_at"].clone();

    assert_json(response, status, expected);
}

fn build(pool: PgPool) -> (Router, Arc<FakeDiscord>) {
    let discord = fake();
    (vc_api::router(state(pool, discord.clone())), discord)
}

async fn claim_token(pool: &PgPool, user: i32) -> String {
    mint(pool, user, &["vc.claim"]).await
}

// --- rejected requests -----------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_patch_without_a_status_or_metadata_is_400(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch(app, CLAIM1, &token, json!({})).await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "must_supply_valid_status" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_patch_with_an_unknown_status_is_400(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch(app, CLAIM1, &token, json!({ "status": "nyan!" })).await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "must_supply_valid_status" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_numeric_claim_id_is_404(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch_with_raw_id(app, "abc", &token, json!({ "status": "approved" })).await;

    assert_json(
        &response,
        404,
        json!({ "error": "not_found", "error_description": "not_found" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_claim_is_404(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch(app, -1, &token, json!({ "status": "approved" })).await;

    assert_json(
        &response,
        404,
        json!({ "error": "not_found", "error_description": "not_found" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_as_the_claimant_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch(app, CLAIM1, &token, json!({ "status": "approved" })).await;

    assert_json(
        &response,
        403,
        json!({ "error": "forbidden", "error_description": "invalid_operator" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn denying_as_the_claimant_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch(app, CLAIM1, &token, json!({ "status": "denied" })).await;

    assert_json(
        &response,
        403,
        json!({ "error": "forbidden", "error_description": "invalid_operator" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancelling_as_the_payer_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER2).await;
    let (app, _) = build(pool.clone());

    let response = patch(app, CLAIM1, &token, json!({ "status": "canceled" })).await;

    assert_json(
        &response,
        403,
        json!({ "error": "forbidden", "error_description": "invalid_operator" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unrelated_user_gets_invalid_operator(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, UNRELATED).await;
    let (app, _) = build(pool.clone());

    for status in ["approved", "denied", "canceled"] {
        let response = patch(app.clone(), CLAIM1, &token, json!({ "status": status })).await;

        assert_json(
            &response,
            403,
            json!({ "error": "forbidden", "error_description": "invalid_operator" }),
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_without_the_claim_scope_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let discord = fake();
    let token = mint(&pool, USER2, &["vc.pay"]).await;

    for status in ["approved", "denied", "canceled"] {
        let response = patch(
            vc_api::router(state(pool.clone(), discord.clone())),
            CLAIM1,
            &token,
            json!({ "status": status }),
        )
        .await;

        assert_json(
            &response,
            403,
            json!({ "error": "invalid_token", "error_description": "permission_denied" }),
        );
    }
}

// --- successful transitions ------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_payer_can_approve_and_the_money_moves(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER2).await;
    let (app, _) = build(pool.clone());

    let before_claimant = amount(&pool, USER1, CURRENCY_ID).await.unwrap_or(0);
    let before_payer = amount(&pool, USER2, CURRENCY_ID).await.unwrap_or(0);

    let response = patch(app, CLAIM1, &token, json!({ "status": "approved" })).await;

    assert_claim(
        &response,
        200,
        claim_json(CLAIM1, "500", USER1, USER2, "approved"),
    );

    assert_eq!(
        amount(&pool, USER1, CURRENCY_ID).await.unwrap_or(0),
        before_claimant + 500,
        "the claimant receives the amount"
    );
    assert_eq!(
        amount(&pool, USER2, CURRENCY_ID).await.unwrap_or(0),
        before_payer - 500,
        "the payer pays the amount"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unaffordable_approval_is_a_conflict(pool: PgPool) {
    fixture(&pool).await;
    // claim 2 is user2 -> user1 for more than user1 holds, and user1 is the payer.
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch(app, CLAIM2, &token, json!({ "status": "approved" })).await;

    assert_json(
        &response,
        409,
        json!({ "error": "conflict", "error_info": "not_enough_amount" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_an_approved_claim_is_a_conflict(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER2).await;
    let (app, _) = build(pool.clone());

    let response = patch(app, CLAIM3, &token, json!({ "status": "approved" })).await;

    assert_json(
        &response,
        409,
        json!({ "error": "conflict", "error_info": "invalid_status" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_payer_can_deny_without_moving_money(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER2).await;
    let (app, _) = build(pool.clone());

    let before_claimant = amount(&pool, USER1, CURRENCY_ID).await.unwrap_or(0);

    let response = patch(app, CLAIM1, &token, json!({ "status": "denied" })).await;

    assert_claim(
        &response,
        200,
        claim_json(CLAIM1, "500", USER1, USER2, "denied"),
    );
    assert_eq!(
        amount(&pool, USER1, CURRENCY_ID).await.unwrap_or(0),
        before_claimant,
        "denying moves no money"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_claimant_can_cancel_without_moving_money(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let before_payer = amount(&pool, USER2, CURRENCY_ID).await.unwrap_or(0);

    let response = patch(app, CLAIM1, &token, json!({ "status": "canceled" })).await;

    assert_claim(
        &response,
        200,
        claim_json(CLAIM1, "500", USER1, USER2, "canceled"),
    );
    assert_eq!(
        amount(&pool, USER2, CURRENCY_ID).await.unwrap_or(0),
        before_payer,
        "cancelling moves no money"
    );
}

// --- metadata-only updates -------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn metadata_can_be_added_without_changing_the_status(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch(app, CLAIM1, &token, json!({ "metadata": { "a": "b" } })).await;

    let mut expected = claim_json(CLAIM1, "500", USER1, USER2, "pending");
    expected["metadata"] = json!({ "a": "b" });
    assert_claim(&response, 200, expected);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn metadata_can_be_merged_and_a_null_value_deletes_a_key(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch(
        app.clone(),
        CLAIM1,
        &token,
        json!({ "metadata": { "a": "b", "e": "f", "d": "xxx" } }),
    )
    .await;
    assert_eq!(response.status, 200);

    let response = patch(
        app,
        CLAIM1,
        &token,
        json!({ "metadata": { "a": "c", "x": "y", "d": null } }),
    )
    .await;

    let mut expected = claim_json(CLAIM1, "500", USER1, USER2, "pending");
    expected["metadata"] = json!({ "a": "c", "e": "f", "x": "y" });
    assert_claim(&response, 200, expected);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_explicit_null_metadata_deletes_everything(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER1).await;
    let (app, _) = build(pool.clone());

    let response = patch(
        app.clone(),
        CLAIM1,
        &token,
        json!({ "metadata": { "d": "xxx" } }),
    )
    .await;
    assert_eq!(response.status, 200);

    let response = patch(app, CLAIM1, &token, json!({ "metadata": null })).await;

    assert_claim(
        &response,
        200,
        claim_json(CLAIM1, "500", USER1, USER2, "pending"),
    );
}

/// Each party only ever sees their own metadata, including after an update.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn metadata_is_private_per_user(pool: PgPool) {
    fixture(&pool).await;
    let token1 = claim_token(&pool, USER1).await;
    let token2 = claim_token(&pool, USER2).await;
    let (app, _) = build(pool.clone());

    let response = patch(
        app.clone(),
        CLAIM1,
        &token1,
        json!({ "metadata": { "owner": "user1" } }),
    )
    .await;
    assert_eq!(response.status, 200);

    let response = patch(
        app,
        CLAIM1,
        &token2,
        json!({ "metadata": { "owner_": "user2" } }),
    )
    .await;

    let mut expected = claim_json(CLAIM1, "500", USER1, USER2, "pending");
    expected["metadata"] = json!({ "owner_": "user2" });
    assert_claim(&response, 200, expected);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_status_transition_merges_metadata(pool: PgPool) {
    fixture(&pool).await;
    let token = claim_token(&pool, USER2).await;
    let (app, _) = build(pool.clone());

    // The payer records pending data first, then approves and clears it.
    let response = patch(
        app.clone(),
        CLAIM1,
        &token,
        json!({ "metadata": { "data": "27", "pending_data": "xxxx" } }),
    )
    .await;
    assert_eq!(response.status, 200);

    let response = patch(
        app,
        CLAIM1,
        &token,
        json!({ "status": "approved", "metadata": { "transaction_id": "abcd1234", "pending_data": null } }),
    )
    .await;

    let mut expected = claim_json(CLAIM1, "500", USER1, USER2, "approved");
    expected["metadata"] = json!({ "data": "27", "transaction_id": "abcd1234" });
    assert_claim(&response, 200, expected);
}

/// Unlike the PATCH helper, this one does not parse the id, so it can exercise a
/// non-numeric path segment.
async fn patch_with_raw_id(app: Router, id: &str, token: &str, body: Value) -> Response {
    let request = axum::http::Request::builder()
        .method("PATCH")
        .uri(format!("/api/v2/users/@me/claims/{id}"))
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("encode body"),
        ))
        .expect("request");

    let response = app.oneshot(request).await.expect("router response");
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
