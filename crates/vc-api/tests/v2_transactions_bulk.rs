//! Contract tests for the array body of `POST /api/v2/users/@me/transactions`,
//! ported from `test/.../user_transactions/pay/bulk/*.exs`.

mod support;

use axum::Router;
use axum::http::HeaderValue;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{Response, fake, insert_asset, insert_currency, insert_user, mint, state};
use tower::ServiceExt;

const GUILD1: i64 = 900_000_000_000_000_001;
const GUILD2: i64 = 900_000_000_000_000_002;
const NYAN: i64 = 1;
const WAN: i64 = 2;
const USER1: i32 = 1;
const USER2: i32 = 2;
const DISCORD1: i64 = 100_000_000_000_000_001;
const DISCORD2: i64 = 100_000_000_000_000_002;
const STRANGER_A: i64 = 100_000_000_000_000_009;
const STRANGER_B: i64 = 100_000_000_000_000_010;
const STRANGER_C: i64 = 100_000_000_000_000_011;

/// `setup_money/1`: user 1 holds 199500 nyan, user 2 holds 1000 nyan and
/// 200000 wan.
async fn fixture(pool: &PgPool) {
    insert_user(pool, USER1, DISCORD1).await;
    insert_user(pool, USER2, DISCORD2).await;
    insert_currency(pool, NYAN, "nyan", "nyan", GUILD1, 500).await;
    insert_currency(pool, WAN, "wan", "wan", GUILD2, 1000).await;
    insert_asset(pool, USER1, NYAN, 199_500).await;
    insert_asset(pool, USER2, NYAN, 1_000).await;
    insert_asset(pool, USER2, WAN, 200_000).await;
}

fn build(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

async fn amount_of(pool: &PgPool, user: i32, currency: i64) -> i64 {
    sqlx::query!(
        "SELECT amount FROM assets WHERE user_id = $1 AND currency_id = $2",
        i64::from(user),
        currency
    )
    .fetch_optional(pool)
    .await
    .expect("read asset")
    .and_then(|row| row.amount)
    .unwrap_or(0)
}

async fn amount_of_discord(pool: &PgPool, discord_id: i64, currency: i64) -> i64 {
    sqlx::query!(
        "SELECT a.amount FROM assets a
           JOIN users u ON u.id = a.user_id
          WHERE u.discord_id = $1 AND a.currency_id = $2",
        discord_id,
        currency
    )
    .fetch_optional(pool)
    .await
    .expect("read asset")
    .and_then(|row| row.amount)
    .unwrap_or(0)
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

fn entry(unit: &str, receiver: i64, amount: &str) -> Value {
    json!({
        "unit": unit,
        "receiver_discord_id": receiver.to_string(),
        "amount": amount,
    })
}

fn idempotency_status(response: &Response) -> Option<&str> {
    response
        .headers
        .get("idempotency-status")
        .and_then(|value| value.to_str().ok())
}

// --- bulk payments ---------------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_without_the_pay_scope_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["oauth2.register"]).await;

    let response = send(build(pool), &token, json!([]), None).await;

    assert_eq!(response.status, 403, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "insufficient_scope", "error_description": "token_verification_failed" })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_empty_array_is_accepted(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(build(pool), &token, json!([]), None).await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(response.body, json!({}));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn one_entry_pays_once(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(
        build(pool.clone()),
        &token,
        json!([entry("nyan", DISCORD2, "20")]),
        None,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(amount_of(&pool, USER1, NYAN).await, 199_500 - 20);
    assert_eq!(amount_of(&pool, USER2, NYAN).await, 1_000 + 20);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn entries_to_unknown_users_create_them(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(
        build(pool.clone()),
        &token,
        json!([
            entry("nyan", STRANGER_A, "20"),
            entry("nyan", STRANGER_B, "30"),
        ]),
        None,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(amount_of(&pool, USER1, NYAN).await, 199_500 - 50);
    assert_eq!(amount_of_discord(&pool, STRANGER_A, NYAN).await, 20);
    assert_eq!(amount_of_discord(&pool, STRANGER_B, NYAN).await, 30);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn entries_can_mix_known_and_unknown_users(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(
        build(pool.clone()),
        &token,
        json!([
            entry("nyan", DISCORD2, "20"),
            entry("nyan", STRANGER_A, "30"),
        ]),
        None,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(amount_of(&pool, USER1, NYAN).await, 199_500 - 50);
    assert_eq!(amount_of(&pool, USER2, NYAN).await, 1_000 + 20);
    assert_eq!(amount_of_discord(&pool, STRANGER_A, NYAN).await, 30);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn entries_can_mix_currencies(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER2, &["vc.pay"]).await;

    let response = send(
        build(pool.clone()),
        &token,
        json!([
            entry("nyan", STRANGER_A, "20"),
            entry("wan", STRANGER_B, "30"),
        ]),
        None,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(amount_of(&pool, USER2, NYAN).await, 1_000 - 20);
    assert_eq!(amount_of(&pool, USER2, WAN).await, 200_000 - 30);
    assert_eq!(amount_of_discord(&pool, STRANGER_A, NYAN).await, 20);
    assert_eq!(amount_of_discord(&pool, STRANGER_B, WAN).await, 30);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn four_entries_can_mix_currencies_and_users(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER2, &["vc.pay"]).await;

    let response = send(
        build(pool.clone()),
        &token,
        json!([
            entry("nyan", STRANGER_A, "20"),
            entry("nyan", STRANGER_B, "20"),
            entry("wan", STRANGER_B, "30"),
            entry("wan", STRANGER_C, "10"),
        ]),
        None,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(amount_of(&pool, USER2, NYAN).await, 1_000 - 40);
    assert_eq!(amount_of(&pool, USER2, WAN).await, 200_000 - 40);
    assert_eq!(amount_of_discord(&pool, STRANGER_A, NYAN).await, 20);
    assert_eq!(amount_of_discord(&pool, STRANGER_B, NYAN).await, 20);
    assert_eq!(amount_of_discord(&pool, STRANGER_B, WAN).await, 30);
    assert_eq!(amount_of_discord(&pool, STRANGER_C, WAN).await, 10);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unaffordable_entry_rolls_the_whole_request_back(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(
        build(pool.clone()),
        &token,
        json!([
            entry("nyan", DISCORD2, "1000000"),
            entry("nyan", STRANGER_A, "30"),
        ]),
        None,
    )
    .await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "conflict", "error_info": "not_enough_amount" })
    );

    // Nothing was applied, including the entry that came first.
    assert_eq!(amount_of(&pool, USER1, NYAN).await, 199_500);
    assert_eq!(amount_of(&pool, USER2, NYAN).await, 1_000);
    assert_eq!(amount_of_discord(&pool, STRANGER_A, NYAN).await, 0);
    assert!(
        vc_core::user::find_by_discord_id(&pool, STRANGER_A)
            .await
            .unwrap()
            .is_none()
    );
}

/// The entries are affordable one at a time but not together: 199500 < 200000.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn entries_are_judged_together_not_apart(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(
        build(pool.clone()),
        &token,
        json!([
            entry("nyan", DISCORD2, "100000"),
            entry("nyan", STRANGER_A, "100000"),
        ]),
        None,
    )
    .await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(amount_of(&pool, USER1, NYAN).await, 199_500);
    assert_eq!(amount_of(&pool, USER2, NYAN).await, 1_000);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_whole_balance_can_be_paid_in_one_entry(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;
    let balance = amount_of(&pool, USER1, NYAN).await;

    let response = send(
        build(pool.clone()),
        &token,
        json!([entry("nyan", DISCORD2, &balance.to_string())]),
        None,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(amount_of(&pool, USER1, NYAN).await, 0);
    assert_eq!(amount_of(&pool, USER2, NYAN).await, 1_000 + balance);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn one_more_than_the_balance_is_a_conflict(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;
    let balance = amount_of(&pool, USER1, NYAN).await;

    let response = send(
        build(pool),
        &token,
        json!([entry("nyan", DISCORD2, &(balance + 1).to_string())]),
        None,
    )
    .await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "conflict", "error_info": "not_enough_amount" })
    );
}

// --- entry validation ------------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bad_entry_reports_its_tag_and_index(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let cases = [
        (
            json!([{ "unit": "nyan", "receiver_discord_id": DISCORD2.to_string() }]),
            "invalid_amount_at_0",
        ),
        (
            json!([
                entry("nyan", DISCORD2, "20"),
                { "unit": "nyan", "amount": "20" },
            ]),
            "invalid_receiver_discord_id_at_1",
        ),
        (
            json!([{ "receiver_discord_id": DISCORD2.to_string(), "amount": "20" }]),
            "invalid_unit_at_0",
        ),
    ];

    for (body, description) in cases {
        let response = send(build(pool.clone()), &token, body, None).await;

        assert_eq!(response.status, 400, "body: {}", response.body);
        assert_eq!(
            response.body,
            json!({ "error": "invalid_request", "error_description": description })
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_unit_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = send(
        build(pool),
        &token,
        json!([entry("dollar", DISCORD2, "20")]),
        None,
    )
    .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "invalid_request", "error_info": "not_found_currency" })
    );
}

// --- idempotency -----------------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_key_covers_the_whole_bulk_request(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER2, &["vc.pay"]).await;
    let key = || HeaderValue::from_static("\"1dEmP0104Ke1\"");

    let body = json!([
        entry("nyan", STRANGER_A, "20"),
        entry("nyan", STRANGER_B, "20"),
        entry("wan", STRANGER_B, "30"),
        entry("wan", STRANGER_C, "10"),
    ]);

    let first = send(build(pool.clone()), &token, body.clone(), Some(key())).await;
    let second = send(build(pool.clone()), &token, body, Some(key())).await;

    assert_eq!(first.status, 201, "body: {}", first.body);
    assert_eq!(second.status, 201, "body: {}", second.body);
    assert_eq!(second.body, first.body);

    assert_eq!(idempotency_status(&first), Some("OK"));
    assert_eq!(idempotency_status(&second), Some("Duplicate"));

    // The batch is applied once.
    assert_eq!(amount_of(&pool, USER2, NYAN).await, 1_000 - 40);
    assert_eq!(amount_of(&pool, USER2, WAN).await, 200_000 - 40);
    assert_eq!(amount_of_discord(&pool, STRANGER_A, NYAN).await, 20);
    assert_eq!(amount_of_discord(&pool, STRANGER_B, NYAN).await, 20);
    assert_eq!(amount_of_discord(&pool, STRANGER_B, WAN).await, 30);
    assert_eq!(amount_of_discord(&pool, STRANGER_C, WAN).await, 10);
}

// Each entry fits bigint, but the batch cannot fit any sender's balance.
async fn assert_overflow_is_refused(pool: PgPool, receivers: [i64; 3]) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;
    let amounts = [i64::MAX, i64::MAX, 10];
    let body = Value::Array(receivers.into_iter().zip(amounts).map(|(receiver, amount)| {
        json!({"unit": "nyan", "receiver_discord_id": receiver.to_string(), "amount": amount.to_string()})
    }).collect());
    let response = send(build(pool.clone()), &token, body, None).await;
    support::assert_json(
        &response,
        409,
        json!({"error":"conflict", "error_info":"not_enough_amount"}),
    );
    assert_eq!(amount_of(&pool, USER1, NYAN).await, 199_500);
    for receiver in receivers {
        assert_eq!(amount_of_discord(&pool, receiver, NYAN).await, 0);
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn overflowing_total_across_receivers_is_refused(pool: PgPool) {
    assert_overflow_is_refused(pool, [STRANGER_A, STRANGER_B, STRANGER_C]).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn overflowing_total_for_one_receiver_is_refused(pool: PgPool) {
    assert_overflow_is_refused(pool, [STRANGER_A; 3]).await;
}
