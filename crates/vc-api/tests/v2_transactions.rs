//! Contract tests for `POST /api/v2/users/@me/transactions` (the single-payment
//! form and its idempotency layer), ported from
//! `test/.../user_transactions/pay/single/*.exs`.

mod support;

use axum::Router;
use axum::http::HeaderValue;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{Response, fake, insert_asset, insert_currency, insert_user, mint, state};
use tower::ServiceExt;

const GUILD: i64 = 900_000_000_000_000_001;
const CURRENCY_ID: i64 = 1;
const USER1: i32 = 1;
const USER2: i32 = 2;
const DISCORD1: i64 = 100_000_000_000_000_001;
const DISCORD2: i64 = 100_000_000_000_000_002;

/// The Elixir suite's default key, including its trailing `0xFF` byte, which the
/// validator's `\x80-\xFF` range permits.
const KEY: &str = "1dEmP0104Ke1\u{FF}";

async fn fixture(pool: &PgPool) {
    insert_user(pool, USER1, DISCORD1).await;
    insert_user(pool, USER2, DISCORD2).await;
    insert_currency(pool, CURRENCY_ID, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, USER1, CURRENCY_ID, 199_500).await;
    insert_asset(pool, USER2, CURRENCY_ID, 1_000).await;
}

fn build(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

async fn amount(pool: &PgPool, user: i32) -> i64 {
    sqlx::query!(
        "SELECT amount FROM assets WHERE user_id = $1 AND currency_id = $2",
        i64::from(user),
        CURRENCY_ID
    )
    .fetch_optional(pool)
    .await
    .expect("read asset")
    .and_then(|row| row.amount)
    .unwrap_or(0)
}

fn key_header(key: &str) -> HeaderValue {
    HeaderValue::from_bytes(format!("\"{key}\"").as_bytes()).expect("a valid header value")
}

/// The header value verbatim, for keys that are deliberately malformed.
fn raw_header(value: &str) -> HeaderValue {
    HeaderValue::from_bytes(value.as_bytes()).expect("a valid header value")
}

async fn send(
    app: Router,
    token: &str,
    body: Value,
    idempotency_key: Option<HeaderValue>,
) -> Response {
    let mut builder = axum::http::Request::builder()
        .method("POST")
        .uri("/api/v2/users/@me/transactions")
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"));

    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }

    let request = builder
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

fn idempotency_status(response: &Response) -> Option<&str> {
    response
        .headers
        .get("idempotency-status")
        .and_then(|value| value.to_str().ok())
}

fn pay(amount: &str) -> Value {
    json!({
        "unit": "nyan",
        "receiver_discord_id": DISCORD2.to_string(),
        "amount": amount,
    })
}

// --- single payment --------------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_without_the_pay_scope_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["oauth2.register"]).await;

    let response = send(build(pool), &token, pay("20"), None).await;

    assert_eq!(response.status, 403, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "insufficient_scope", "error_description": "token_verification_failed" })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_empty_object_is_missing_a_parameter(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(build(pool), &token, json!({}), None).await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "invalid_request", "error_description": "missing_parameter" })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_valid_payment_moves_the_money(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;
    let app = build(pool.clone());

    let response = send(app, &token, pay("20"), None).await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(response.body, json!({}));
    assert_eq!(idempotency_status(&response), Some("Not Requested"));

    assert_eq!(amount(&pool, USER1).await, 199_500 - 20);
    assert_eq!(amount(&pool, USER2).await, 1_000 + 20);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_an_unknown_user_creates_their_account(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;
    let stranger = 100_000_000_000_000_009_i64;

    let response = send(
        build(pool.clone()),
        &token,
        json!({
            "unit": "nyan",
            "receiver_discord_id": stranger.to_string(),
            "amount": "20",
        }),
        None,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);

    let created = sqlx::query!(
        "SELECT a.amount FROM assets a
           JOIN users u ON u.id = a.user_id
          WHERE u.discord_id = $1 AND a.currency_id = $2",
        stranger,
        CURRENCY_ID
    )
    .fetch_one(&pool)
    .await
    .expect("the receiver has an asset");

    assert_eq!(created.amount, Some(20));
    assert_eq!(amount(&pool, USER1).await, 199_500 - 20);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_more_than_the_balance_is_a_conflict(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(build(pool), &token, pay("1000000"), None).await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "conflict", "error_info": "not_enough_amount" })
    );
}

/// Paying the whole balance deletes the sender's asset row, so a zero balance is
/// the absence of a row.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_whole_balance_can_be_paid(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;
    let balance = amount(&pool, USER1).await;

    let response = send(build(pool.clone()), &token, pay(&balance.to_string()), None).await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(amount(&pool, USER1).await, 0);
    assert_eq!(amount(&pool, USER2).await, 1_000 + balance);

    let rows = sqlx::query_scalar!(
        "SELECT count(*) FROM assets WHERE user_id = $1 AND currency_id = $2",
        i64::from(USER1),
        CURRENCY_ID
    )
    .fetch_one(&pool)
    .await
    .expect("count assets");

    assert_eq!(rows, Some(0), "the emptied row is gone");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn one_more_than_the_balance_is_a_conflict(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;
    let balance = amount(&pool, USER1).await;

    let response = send(build(pool), &token, pay(&(balance + 1).to_string()), None).await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "conflict", "error_info": "not_enough_amount" })
    );
}

// --- idempotency -----------------------------------------------------------

async fn pay_with_key(pool: &PgPool, token: &str, amount: &str, key: &str) -> Response {
    send(
        build(pool.clone()),
        token,
        pay(amount),
        Some(key_header(key)),
    )
    .await
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_idempotency_key_without_the_scope_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["oauth2.register"]).await;

    let response = send(build(pool), &token, json!([]), Some(key_header(KEY))).await;

    assert_eq!(response.status, 403, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "insufficient_scope", "error_description": "token_verification_failed" })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_repeated_key_replays_the_first_response(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let first = pay_with_key(&pool, &token, "20", KEY).await;
    let second = pay_with_key(&pool, &token, "20", KEY).await;

    assert_eq!(first.status, 201, "body: {}", first.body);
    assert_eq!(second.status, 201, "body: {}", second.body);
    assert_eq!(second.body, first.body);

    assert_eq!(idempotency_status(&first), Some("OK"));
    assert_eq!(idempotency_status(&second), Some("Duplicate"));

    // The payment is applied once, not twice.
    assert_eq!(amount(&pool, USER1).await, 199_500 - 20);
    assert_eq!(amount(&pool, USER2).await, 1_000 + 20);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_key_is_independent_for_each_user(pool: PgPool) {
    fixture(&pool).await;
    let token1 = mint(&pool, USER1, &["vc.pay"]).await;
    let token2 = mint(&pool, USER2, &["vc.pay"]).await;

    let first = pay_with_key(&pool, &token1, "40", KEY).await;
    let second = send(
        build(pool.clone()),
        &token2,
        json!({
            "unit": "nyan",
            "receiver_discord_id": DISCORD1.to_string(),
            "amount": "20",
        }),
        Some(key_header(KEY)),
    )
    .await;

    assert_eq!(first.status, 201, "body: {}", first.body);
    assert_eq!(second.status, 201, "body: {}", second.body);
    assert_eq!(idempotency_status(&first), Some("OK"));
    assert_eq!(idempotency_status(&second), Some("OK"));

    assert_eq!(amount(&pool, USER1).await, 199_500 - 40 + 20);
    assert_eq!(amount(&pool, USER2).await, 1_000 + 40 - 20);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_error_response_is_cached_too(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let first = pay_with_key(&pool, &token, "1000000", KEY).await;
    let second = pay_with_key(&pool, &token, "1000000", KEY).await;

    assert_eq!(first.status, 409, "body: {}", first.body);
    assert_eq!(second.status, 409, "body: {}", second.body);
    assert_eq!(
        second.body,
        json!({ "error": "conflict", "error_info": "not_enough_amount" })
    );

    assert_eq!(idempotency_status(&first), Some("OK"));
    assert_eq!(idempotency_status(&second), Some("Duplicate"));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn different_keys_are_applied_separately(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let first = pay_with_key(&pool, &token, "40", KEY).await;
    let second = pay_with_key(&pool, &token, "20", "nyan").await;

    assert_eq!(first.status, 201, "body: {}", first.body);
    assert_eq!(second.status, 201, "body: {}", second.body);
    assert_eq!(idempotency_status(&first), Some("OK"));
    assert_eq!(idempotency_status(&second), Some("OK"));

    assert_eq!(amount(&pool, USER1).await, 199_500 - 60);
    assert_eq!(amount(&pool, USER2).await, 1_000 + 60);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unquoted_key_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(build(pool), &token, pay("20"), Some(raw_header("unquoted"))).await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "invalid_request", "error_description": "invalid_idempotency_key" })
    );
}

/// A body that cannot become a payment does not spend the key: the client fixes
/// the value, sends the same key again, and that request is the payment rather
/// than a replay of its own typo.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_body_that_does_not_parse_does_not_spend_the_key(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let malformed = pay_with_key(&pool, &token, "ten", KEY).await;

    assert_eq!(malformed.status, 400, "body: {}", malformed.body);
    assert_eq!(
        malformed.body["error_description"],
        "invalid_format_of_convert_amount"
    );
    assert_eq!(idempotency_status(&malformed), None, "nothing was claimed");
    assert_eq!(
        claimed(&pool, USER1, KEY).await,
        0,
        "and no row was left behind"
    );

    let corrected = pay_with_key(&pool, &token, "20", KEY).await;

    assert_eq!(corrected.status, 201, "body: {}", corrected.body);
    assert_eq!(idempotency_status(&corrected), Some("OK"));
    assert_eq!(amount(&pool, USER1).await, 199_500 - 20);
    assert_eq!(amount(&pool, USER2).await, 1_000 + 20);
    assert_eq!(claimed(&pool, USER1, KEY).await, 1, "now it is claimed");
}

/// A failure is not an answer: the payment's transaction rolled back, nothing
/// moved, and the key goes back so the caller can try the same payment again with
/// the key it chose. The table the payment reads is dropped rather than the
/// failure simulated, so this is the real path.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_failure_gives_the_key_back(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    sqlx::query("DROP TABLE assets CASCADE")
        .execute(&pool)
        .await
        .expect("the table a payment reads");

    let failed = pay_with_key(&pool, &token, "20", KEY).await;

    assert_eq!(failed.status, 500, "body: {}", failed.body);
    assert_eq!(idempotency_status(&failed), None, "not the layer's answer");
    assert_eq!(claimed(&pool, USER1, KEY).await, 0, "and the key went back");

    let retried = pay_with_key(&pool, &token, "20", KEY).await;

    assert_eq!(retried.status, 500, "body: {}", retried.body);
    assert_eq!(
        idempotency_status(&retried),
        None,
        "attempted again rather than read back — a replay would carry Duplicate"
    );
    assert_eq!(claimed(&pool, USER1, KEY).await, 0, "and released again");
}

/// The rows a key has in the layer's own table, which is what says whether a
/// request claimed it.
async fn claimed(pool: &PgPool, account: i32, key: &str) -> i64 {
    sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM payments_idempotency
          WHERE idempotency_key = $1 AND user_id = $2",
        key.as_bytes().to_vec(),
        i64::from(account)
    )
    .fetch_one(pool)
    .await
    .expect("the layer's table")
}
