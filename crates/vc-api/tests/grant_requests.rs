//! `/oauth2/clients/@me/grant-requests`: where an application asks a guild for a
//! permission, and reads back what the guild said.
//!
//! Additions rather than ports — the Elixir never gave an application a call of
//! its own that could ask, because a grant was only ever a side effect of redeeming
//! an authorization code. The shape is the endpoints' around it: the application
//! token and the `oauth2.register` scope, a snowflake written as a string, and ids
//! answered the same way.

mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{Response, account_of, fake, insert_application, mint, mint_app, state};
use tower::ServiceExt;

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const GUILD: i64 = 900_000_000_000_000_001;

async fn request(app: axum::Router, method: &str, token: Option<&str>, body: Value) -> Response {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri("/oauth2/clients/@me/grant-requests")
        .header("accept", "application/json");

    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }

    if !body.is_null() {
        builder = builder.header("content-type", "application/json");
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

async fn fixture(pool: &PgPool) -> (i64, String) {
    support::insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(pool, OWNER_DISCORD_ID, "mine").await;
    let account = account_of(pool, application).await;
    let token = mint_app(pool, account, &["oauth2.register"]).await;

    (application, token)
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_may_ask_a_guild(pool: PgPool) {
    let (application, token) = fixture(&pool).await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({
            "id": response.body["id"],
            "guild_id": GUILD.to_string(),
            "status": "pending",
        })
    );

    let row = sqlx::query!(
        "SELECT guild_id, status FROM grant_requests WHERE application_id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the request");

    assert_eq!(row.guild_id, GUILD);
    assert_eq!(row.status, "pending");
}

/// Asking twice is the same ask: a second request while one is pending keeps the
/// request that is there.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_second_ask_while_one_is_pending_keeps_it(pool: PgPool) {
    let (_, token) = fixture(&pool).await;

    let first = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    let second = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    assert_eq!(second.status, 201, "body: {}", second.body);
    assert_eq!(second.body["id"], first.body["id"], "the same ask");

    let rows = sqlx::query_scalar!("SELECT count(*) AS \"count!\" FROM grant_requests")
        .fetch_one(&pool)
        .await
        .expect("the rows");

    assert_eq!(rows, 1);
}

/// What the application made of what was asked: the guild's answer, answered or
/// not, which it polls for before exchanging a guild token.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_reads_back_what_was_asked(pool: PgPool) {
    let (application, token) = fixture(&pool).await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    vc_core::grant::decide_request(
        &pool,
        vc_core::grant::requests_of(&pool, application)
            .await
            .expect("the requests")[0]
            .id,
        GUILD,
        true,
        &[],
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("the answer");

    let response = request(
        vc_api::router(state(pool, fake())),
        "GET",
        Some(&token),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!([{
            "id": response.body[0]["id"],
            "guild_id": GUILD.to_string(),
            "status": "approved",
        }])
    );
}

/// The whole of it without a browser: the application asks, the guild answers in
/// Discord, and the token a guild id is exchanged for issues from the pool.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_answer_lets_the_application_issue(pool: PgPool) {
    let (application, token) = fixture(&pool).await;
    support::insert_currency(&pool, 1, "nyan", "nyan", GUILD, 500).await;
    support::insert_user(&pool, 101, 100_000_000_000_000_002).await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    let request_id = vc_core::grant::requests_of(&pool, application)
        .await
        .expect("the requests")[0]
        .id;

    vc_core::grant::decide_request(
        &pool,
        request_id,
        GUILD,
        true,
        &[vc_core::application::ISSUE],
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("the answer");

    let guild_token = support::mint_guild_token(&pool, application, GUILD).await;

    let response = axum::http::Request::builder()
        .method("POST")
        .uri("/api/v2/currencies/issue")
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {guild_token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "receiver_discord_id": "100000000000000002",
                "amount": "100",
            }))
            .expect("encode body"),
        ))
        .expect("request");

    let response = vc_api::router(state(pool, fake()))
        .oneshot(response)
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 201);
}

/// A user token asks with no application behind it, and nothing says the caller
/// *is* the application a grant would be written for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_user_token_is_refused(pool: PgPool) {
    support::insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = request(
        vc_api::router(state(pool, fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    assert_eq!(response.status, 401, "body: {}", response.body);
    assert_eq!(response.body["error"], "invalid_kind");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_without_the_scope_is_refused(pool: PgPool) {
    support::insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let account = account_of(&pool, application).await;
    let token = mint_app(&pool, account, &["vc.pay"]).await;

    let response = request(
        vc_api::router(state(pool, fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    assert_eq!(response.status, 403, "body: {}", response.body);
    assert_eq!(response.body["error"], "insufficient_scope");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_id_that_is_not_a_snowflake_is_400(pool: PgPool) {
    let (_, token) = fixture(&pool).await;

    let response = request(
        vc_api::router(state(pool, fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": "not-a-snowflake" }),
    )
    .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(response.body["error"], "invalid_request");
}
