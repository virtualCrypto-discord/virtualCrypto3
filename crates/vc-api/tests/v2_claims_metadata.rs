//! Metadata validation limits, ported from
//! `test/virtualCrypto_web/controllers/api/v2/claim/metadata/update_test.exs`.
//!
//! `Rest.md` puts the content of `error_description` (and even its presence)
//! outside the specification, so these tests assert the status, `error` and
//! `error_description` code, and compare `error_description_details` as a *set* —
//! the order Elixir happens to enumerate them in is not part of the contract.

mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    Response, fake, insert_asset, insert_claim, insert_claim_metadata, insert_currency,
    insert_user, mint, state,
};
use tower::ServiceExt;

const GUILD: i64 = 900_000_000_000_000_001;
const CURRENCY_ID: i64 = 1;
const CLAIM_ID: i64 = 1;
const USER1: i32 = 1;
const USER2: i32 = 2;
const DISCORD1: i64 = 100_000_000_000_000_001;
const DISCORD2: i64 = 100_000_000_000_000_002;

async fn fixture(pool: &PgPool) {
    insert_user(pool, USER1, DISCORD1).await;
    insert_user(pool, USER2, DISCORD2).await;
    insert_currency(pool, CURRENCY_ID, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, USER1, CURRENCY_ID, 199_500).await;
    insert_asset(pool, USER2, CURRENCY_ID, 1_000).await;
    insert_claim(pool, CLAIM_ID, 500, "pending", USER1, USER2, CURRENCY_ID).await;
}

async fn send(pool: PgPool, method: &str, uri: &str, token: &str, body: Value) -> Response {
    let request = axum::http::Request::builder()
        .method(method)
        .uri(uri)
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

async fn post(pool: PgPool, token: &str, body: Value) -> Response {
    send(pool, "POST", "/api/v2/users/@me/claims", token, body).await
}

async fn patch(pool: PgPool, token: &str, body: Value) -> Response {
    send(
        pool,
        "PATCH",
        &format!("/api/v2/users/@me/claims/{CLAIM_ID}"),
        token,
        body,
    )
    .await
}

fn details(response: &Response) -> Vec<String> {
    let mut details: Vec<String> = response.body["error_description_details"]
        .as_array()
        .expect("error_description_details is an array")
        .iter()
        .map(|value| value.as_str().expect("a string").to_string())
        .collect();
    details.sort();

    details
}

fn assert_invalid_metadata(response: &Response, mut expected: Vec<String>) {
    assert_eq!(response.status, 400, "status: {}", response.body);
    assert_eq!(response.body["error"], json!("invalid_request"));
    assert_eq!(
        response.body["error_description"],
        json!("invalid_metadata")
    );

    expected.sort();
    assert_eq!(
        details(response),
        expected,
        "details (order is not contractual)"
    );
}

/// `json!` needs literal keys; these cases build keys at runtime.
fn object(entries: Vec<(String, Value)>) -> Value {
    Value::Object(entries.into_iter().collect())
}

// --- per-entry limits ------------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_over_long_key_and_value_are_rejected_on_create(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = post(
        pool,
        &token,
        json!({
            "payer_discord_id": DISCORD2.to_string(),
            "unit": "nyan",
            "amount": "20",
            "metadata": object(vec![
                ("b".repeat(41), json!("x")),
                ("x".to_string(), json!("b".repeat(501))),
            ]),
        }),
    )
    .await;

    assert_invalid_metadata(
        &response,
        vec![
            format!("too large(max: 40) metadata key({}...)", "b".repeat(41)),
            format!(
                "too large metadata value(max: 500) at x({}...)",
                "b".repeat(501)
            ),
        ],
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_over_long_key_and_value_are_rejected_on_update(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = patch(
        pool,
        &token,
        json!({
            "metadata": object(vec![
                ("x".repeat(41), json!("c")),
                ("x".to_string(), json!("b".repeat(501))),
                ("d".to_string(), Value::Null),
            ]),
        }),
    )
    .await;

    assert_invalid_metadata(
        &response,
        vec![
            format!("too large(max: 40) metadata key({}...)", "x".repeat(41)),
            format!(
                "too large metadata value(max: 500) at x({}...)",
                "b".repeat(501)
            ),
        ],
    );
}

// --- entry-count limits ----------------------------------------------------

fn entries(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 1..=count {
        map.insert(index.to_string(), Value::String(index.to_string()));
    }
    Value::Object(map)
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn too_many_entries_are_rejected_on_create(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = post(
        pool,
        &token,
        json!({
            "payer_discord_id": DISCORD2.to_string(),
            "unit": "nyan",
            "amount": "20",
            "metadata": entries(51),
        }),
    )
    .await;

    assert_invalid_metadata(
        &response,
        vec!["too many entries in metadata(max: 50)".to_string()],
    );
}

/// On an update the application validator only sees the patch, so a 51st entry
/// passes it and the database trigger is what rejects the merge. That path
/// answers with the controller's long message and no details.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn topping_up_past_the_entry_limit_is_rejected_on_update(pool: PgPool) {
    fixture(&pool).await;
    insert_claim_metadata(&pool, CLAIM_ID, USER1, USER2, USER1, entries(50)).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = patch(pool, &token, json!({ "metadata": { "51": "51" } })).await;

    assert_eq!(response.status, 400, "status: {}", response.body);
    assert_eq!(response.body["error"], json!("invalid_request"));
    assert_eq!(
        response.body["error_description"],
        json!(
            "The upper limit of the number of metadata is 50, and it is highly possible that this has been reached. (Maybe for other reasons)"
        )
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn topping_up_to_exactly_the_limit_is_allowed(pool: PgPool) {
    fixture(&pool).await;
    insert_claim_metadata(&pool, CLAIM_ID, USER1, USER2, USER1, entries(49)).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = patch(pool, &token, json!({ "metadata": { "50": "50" } })).await;

    assert_eq!(response.status, 200, "status: {}", response.body);
    assert_eq!(
        response.body["metadata"]
            .as_object()
            .expect("an object")
            .len(),
        50
    );
}
