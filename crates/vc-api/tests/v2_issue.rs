//! `POST /api/v2/currencies/issue`: a guild's pool pays, on the authority of a
//! guild token.
//!
//! No Elixir counterpart. Neither the endpoint nor the token kind existed there,
//! so these are additions rather than ports; what they pin is the contract the new
//! endpoint has — the guild token, the scope that lets it issue, and the three
//! failures the payment endpoint already has words for.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    Money, Response, account_of, currency_by_unit, fake, get_amount, insert_application,
    insert_grant, mint, mint_app, setup_money, state,
};
use tower::ServiceExt;

/// The application's owner, which is a Discord id and nobody's account.
const OWNER: i64 = 100_000_000_000_000_009;
/// The Elixir suite's idempotency key, trailing `0xFF` included.
const KEY: &str = "1dEmP0104Ke1\u{FF}";

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

async fn send(app: Router, token: &str, body: Value, key: Option<&str>) -> Response {
    let mut builder = axum::http::Request::builder()
        .method("POST")
        .uri("/api/v2/currencies/issue")
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"));

    if let Some(key) = key {
        builder = builder.header(
            "idempotency-key",
            axum::http::HeaderValue::from_bytes(format!("\"{key}\"").as_bytes())
                .expect("a header value"),
        );
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

fn issue(receiver: i64, amount: Value) -> Value {
    json!({ "receiver_discord_id": receiver.to_string(), "amount": amount })
}

/// A guild with a currency in its pool, an application, and the token that guild
/// allowed it to issue with.
async fn fixture(pool: &PgPool) -> (Money, i64, String) {
    let money = setup_money(pool).await;
    let application = insert_application(pool, OWNER, "an application").await;
    let token = insert_grant(pool, application, money.guild, &["vc.issue"]).await;

    (money, application, token)
}

fn idempotency_status(response: &Response) -> Option<&str> {
    response
        .headers
        .get("idempotency-status")
        .and_then(|value| value.to_str().ok())
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issuing_pays_the_receiver_and_takes_the_amount_from_the_pool(pool: PgPool) {
    let (money, _, token) = fixture(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;

    let response = send(
        router(pool.clone()),
        &token,
        issue(money.user2, json!("100")),
        None,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "amount": "100", "pool_amount": "400", "unit": &money.unit })
    );
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 100
    );
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .expect("the currency")
            .pool_amount,
        Some(400)
    );
}

/// The history row is the one the command writes, because it is the same domain
/// call underneath.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_issue_is_recorded_as_given(pool: PgPool) {
    let (money, _, token) = fixture(&pool).await;

    send(
        router(pool.clone()),
        &token,
        issue(money.user2, json!("100")),
        None,
    )
    .await;

    let rows = sqlx::query_scalar!("SELECT count(*) AS \"count!\" FROM currency_given_histories")
        .fetch_one(&pool)
        .await
        .expect("count the histories");

    assert_eq!(rows, 1);
}

/// The receiver need not have an account yet, which is the same answer the
/// payment endpoint gives for an unknown receiver.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_receiver_without_an_account_gets_one(pool: PgPool) {
    let (money, _, token) = fixture(&pool).await;
    let receiver = 100_000_000_000_000_099;

    let response = send(
        router(pool.clone()),
        &token,
        issue(receiver, json!("100")),
        None,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(
        get_amount(&pool, receiver, money.currency).await,
        100,
        "the new account holds what was issued"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_grant_without_the_issuing_scope_may_not_issue(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, OWNER, "an application").await;
    let token = insert_grant(&pool, application, money.guild, &["openid"]).await;

    let response = send(router(pool), &token, issue(money.user2, json!("100")), None).await;

    assert_eq!(response.status, 403, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "insufficient_scope", "error_description": "token_verification_failed" })
    );
}

/// The scope is refused before the body is read, which is where the payment
/// endpoint's idempotency layer checks it too.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_key_does_not_make_a_scopeless_token_able_to_issue(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, OWNER, "an application").await;
    let token = insert_grant(&pool, application, money.guild, &["openid"]).await;

    let response = send(
        router(pool),
        &token,
        issue(money.user2, json!("100")),
        Some(KEY),
    )
    .await;

    assert_eq!(response.status, 403, "body: {}", response.body);
}

/// A user token is a JWT and a guild token is a row, so one can never be taken
/// for the other.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_user_token_is_not_a_guild_token(pool: PgPool) {
    let money = setup_money(&pool).await;
    let token = mint(&pool, 1, &["vc.issue"]).await;

    let response = send(router(pool), &token, issue(money.user2, json!("100")), None).await;

    assert_eq!(response.status, 401, "body: {}", response.body);
    assert_eq!(response.body, json!({ "error": "invalid_token" }));
}

/// An application token is not one either, even one that asks for the scope.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_token_is_not_a_guild_token(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, OWNER, "an application").await;
    let account = account_of(&pool, application).await;
    let token = mint_app(&pool, account, &["vc.issue"]).await;

    let response = send(router(pool), &token, issue(money.user2, json!("100")), None).await;

    assert_eq!(response.status, 401, "body: {}", response.body);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_token_is_refused(pool: PgPool) {
    let money = setup_money(&pool).await;
    let token = uuid::Uuid::new_v4().to_string();

    let response = send(router(pool), &token, issue(money.user2, json!("100")), None).await;

    assert_eq!(response.status, 401, "body: {}", response.body);
}

/// An expired row is not a token, and this is the check the JWT extractor does
/// not make — the purge job is what retires those.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_expired_token_is_refused(pool: PgPool) {
    let (money, _, token) = fixture(&pool).await;
    let id = uuid::Uuid::parse_str(&token).expect("a uuid");

    sqlx::query!(
        "UPDATE access_tokens SET expires = $1 WHERE token_id = $2",
        support::utc_now() - time::Duration::hours(2),
        id
    )
    .execute(&pool)
    .await
    .expect("expire the token");

    let response = send(router(pool), &token, issue(money.user2, json!("100")), None).await;

    assert_eq!(response.status, 401, "body: {}", response.body);
}

/// A grant with no guild in it is what a code exchange without a guild id leaves
/// behind, and it is not a guild anything can be done in.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_for_a_grant_without_a_guild_is_refused(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, OWNER, "an application").await;
    let token = insert_grant(&pool, application, 0, &["vc.issue"]).await;

    let response = send(router(pool), &token, issue(money.user2, json!("100")), None).await;

    assert_eq!(response.status, 401, "body: {}", response.body);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn more_than_the_pool_is_refused(pool: PgPool) {
    let (money, _, token) = fixture(&pool).await;

    let response = send(router(pool), &token, issue(money.user2, json!("501")), None).await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "conflict", "error_info": "not_enough_amount" })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_amount_that_is_not_positive_is_refused(pool: PgPool) {
    let (money, _, token) = fixture(&pool).await;

    let response = send(router(pool), &token, issue(money.user2, json!("0")), None).await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "invalid_request", "error_description": "invalid_amount" })
    );
}

/// Unlike the command, the amount is required: there an omitted amount is the
/// whole pool, and a bot that forgot a field should not drain a guild.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_missing_amount_is_missing_parameter(pool: PgPool) {
    let (money, _, token) = fixture(&pool).await;
    let body = json!({ "receiver_discord_id": money.user2.to_string() });

    let response = send(router(pool), &token, body, None).await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "invalid_request", "error_description": "missing_parameter" })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_amount_that_is_not_a_number_is_refused(pool: PgPool) {
    let (money, _, token) = fixture(&pool).await;

    let response = send(
        router(pool),
        &token,
        issue(money.user2, json!("lots")),
        None,
    )
    .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "invalid_request", "error_description": "invalid_format_of_amount" })
    );
}

/// A guild with nothing in its pool is a guild with no currency: one currency per
/// guild, and the lookup is by the guild the token named.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_without_a_currency_is_refused(pool: PgPool) {
    setup_money(&pool).await;
    let application = insert_application(&pool, OWNER, "an application").await;
    let token = insert_grant(&pool, application, 900_000_000_000_000_777, &["vc.issue"]).await;

    let response = send(
        router(pool),
        &token,
        issue(100_000_000_000_000_001, json!("100")),
        None,
    )
    .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "error": "invalid_request", "error_info": "not_found_currency" })
    );
}

/// The key is what makes a retry safe, which for issuing means a retry cannot
/// drain a guild twice.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_same_key_issues_once(pool: PgPool) {
    let (money, _, token) = fixture(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;

    let first = send(
        router(pool.clone()),
        &token,
        issue(money.user2, json!("100")),
        Some(KEY),
    )
    .await;

    assert_eq!(first.status, 201, "body: {}", first.body);
    assert_eq!(idempotency_status(&first), Some("OK"));

    let second = send(
        router(pool.clone()),
        &token,
        issue(money.user2, json!("100")),
        Some(KEY),
    )
    .await;

    assert_eq!(second.status, 201, "body: {}", second.body);
    assert_eq!(second.body, first.body, "the answer is the one it stored");
    assert_eq!(idempotency_status(&second), Some("Duplicate"));
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 100,
        "issued once"
    );
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .expect("the currency")
            .pool_amount,
        Some(400)
    );
}
