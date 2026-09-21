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

async fn device_poll(pool: &PgPool, application: i64, device_code: &str) -> (u16, Value) {
    use base64::Engine;

    let client_id = support::client_id_of(pool, application).await;
    let client_secret = support::client_secret_of(pool, application).await;
    let basic =
        base64::engine::general_purpose::STANDARD.encode(format!("{client_id}:{client_secret}"));

    let response = axum::http::Request::builder()
        .method("POST")
        .uri("/oauth2/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", format!("Basic {basic}"))
        .body(axum::body::Body::from(format!(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code={device_code}"
        )))
        .expect("request");

    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(response)
        .await
        .expect("router response");

    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");

    (status, serde_json::from_slice(&bytes).expect("json body"))
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_may_ask_a_guild(pool: PgPool) {
    let (application, token) = fixture(&pool).await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] }),
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(response.body["guild_id"], Value::Null, "no guild echoes");
    assert_eq!(response.body["status"], Value::Null, "no status either");

    let device_code = response.body["device_code"]
        .as_str()
        .expect("a device code");
    let user_code = response.body["user_code"].as_str().expect("a user code");

    assert_eq!(response.body["expires_in"], 600, "the default lifetime");
    assert!(
        uuid::Uuid::parse_str(device_code).is_ok(),
        "opaque to the device: {device_code}"
    );
    assert_eq!(user_code.len(), 8, "short enough to type: {user_code}");

    let row = sqlx::query!(
        "SELECT guild_id, scopes AS \"scopes!: Vec<String>\", status FROM grant_requests WHERE application_id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the request");

    assert_eq!(row.guild_id, GUILD);
    assert_eq!(row.scopes, ["vc.issue"]);
    assert_eq!(row.status, "pending");
}

/// The scopes are the ask: without them there is nothing to approve, and an
/// unknown one is refused the way the consent screen refuses it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_ask_without_scopes_is_refused(pool: PgPool) {
    let (_, token) = fixture(&pool).await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(response.body["error"], "invalid_request");

    let unknown = request(
        vc_api::router(state(pool, fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.pay"] }),
    )
    .await;

    assert_eq!(unknown.status, 400, "body: {}", unknown.body);
    assert_eq!(unknown.body["error"], "invalid_scope");
}

/// A device poll for a code that never existed is `invalid_grant`, not a hint
/// about what does exist: the application knows its own asks.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_poll_for_an_unknown_code_is_invalid_grant(pool: PgPool) {
    let (application, _) = fixture(&pool).await;

    let (status, body) = device_poll(&pool, application, &uuid::Uuid::new_v4().to_string()).await;

    assert_eq!(status, 400, "body: {body}");
    assert_eq!(body["error"], "invalid_grant");
}

/// An expired ask polls as gone: the device must ask again rather than wait on
/// something the guild will never see.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_poll_for_an_expired_ask_is_invalid_grant(pool: PgPool) {
    let (application, _) = fixture(&pool).await;

    let asked = vc_core::grant::request_grant(
        &pool,
        application,
        GUILD,
        &["vc.issue".to_owned()],
        600,
        time::OffsetDateTime::now_utc() - time::Duration::hours(2),
    )
    .await
    .expect("an old ask");

    let (status, body) = device_poll(&pool, application, &asked.device_code.to_string()).await;

    assert_eq!(status, 400, "body: {body}");
    assert_eq!(body["error"], "invalid_grant");
}

/// A poll without the application's own credentials is no application's poll.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_poll_without_credentials_is_invalid_client(pool: PgPool) {
    let response = axum::http::Request::builder()
        .method("POST")
        .uri("/oauth2/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(axum::body::Body::from(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code=x",
        ))
        .expect("request");

    let response = vc_api::router(state(pool, fake()))
        .oneshot(response)
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 400);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json body");

    assert_eq!(body["error"], "invalid_client");
}

/// Asking twice is the same ask: a second request while one is pending keeps the
/// request that is there — the codes with it, so the poll and the screen agree.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_second_ask_while_one_is_pending_keeps_it(pool: PgPool) {
    let (_, token) = fixture(&pool).await;

    let first = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] }),
    )
    .await;

    let second = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] }),
    )
    .await;

    assert_eq!(second.status, 201, "body: {}", second.body);
    assert_eq!(
        second.body["device_code"], first.body["device_code"],
        "the same ask"
    );
    assert_eq!(
        second.body["user_code"], first.body["user_code"],
        "the guild types the same code"
    );

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
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] }),
    )
    .await;

    let user_code = vc_core::grant::requests_of(&pool, application)
        .await
        .expect("the requests")[0]
        .user_code
        .clone();

    vc_core::grant::decide_request(&pool, &user_code, GUILD, time::OffsetDateTime::now_utc())
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
            "device_code": response.body[0]["device_code"],
            "user_code": user_code,
            "guild_id": GUILD.to_string(),
            "scopes": ["vc.issue"],
            "status": "approved",
            "expires_in": response.body[0]["expires_in"],
        }])
    );
}

/// The whole of it without a browser: the application asks, the guild answers in
/// Discord, and the device poll answers with the guild token that issues.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_answer_lets_the_device_poll_a_guild_token(pool: PgPool) {
    let (application, token) = fixture(&pool).await;
    support::insert_currency(&pool, 1, "nyan", "nyan", GUILD, 500).await;
    support::insert_user(&pool, 101, 100_000_000_000_000_002).await;

    let asked = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] }),
    )
    .await;

    assert_eq!(asked.status, 201, "body: {}", asked.body);

    let device_code = asked.body["device_code"]
        .as_str()
        .expect("a device code")
        .to_owned();
    let user_code = asked.body["user_code"]
        .as_str()
        .expect("a user code")
        .to_owned();

    let _ = application;

    // Before the guild answers: pending, and the device keeps polling.
    let pending = device_poll(&pool, application, &device_code).await;

    assert_eq!(pending.0, 400);
    assert_eq!(pending.1["error"], "authorization_pending");

    // The guild answers in Discord.
    vc_core::grant::decide_request(&pool, &user_code, GUILD, time::OffsetDateTime::now_utc())
        .await
        .expect("the answer");

    // After: the guild token.
    let (status, body) = device_poll(&pool, application, &device_code).await;

    assert_eq!(status, 200, "body: {body}");
    assert_eq!(body["token_type"], "Bearer");

    let guild_token = body["access_token"].as_str().expect("a guild token");

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
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] }),
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
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] }),
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
        json!({ "guild_id": "not-a-snowflake", "scopes": ["vc.issue"] }),
    )
    .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(response.body["error"], "invalid_request");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_expired_ask_can_be_replaced_before_the_next_purge(pool: PgPool) {
    let (application, token) = fixture(&pool).await;
    let now = time::OffsetDateTime::now_utc();
    let old = vc_core::grant::request_grant(
        &pool,
        application,
        GUILD,
        &["openid".to_owned()],
        600,
        now - time::Duration::seconds(601),
    )
    .await
    .unwrap();
    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({"guild_id": GUILD.to_string(), "scopes": ["vc.issue"], "expires_in": 1200}),
    )
    .await;
    assert_eq!(response.status, 201, "{}", response.body);
    assert_eq!(response.body["expires_in"], 1200);
    assert_ne!(response.body["device_code"], old.device_code.to_string());
    assert_ne!(response.body["user_code"], old.user_code);
    assert_eq!(
        device_poll(&pool, application, &old.device_code.to_string())
            .await
            .0,
        400
    );
    let code = response.body["user_code"].as_str().unwrap();
    assert_eq!(
        vc_core::grant::decide_request(&pool, code, GUILD, now)
            .await
            .unwrap(),
        Some(application)
    );
    let device_code = response.body["device_code"].as_str().unwrap();
    assert_eq!(device_poll(&pool, application, device_code).await.0, 200);
    let grants = vc_core::grant::grants_of(&pool, application).await.unwrap();
    assert_eq!(grants[0].scopes, ["vc.issue"]);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn concurrent_replacements_share_one_live_ask_at_expiry(pool: PgPool) {
    let (application, _) = fixture(&pool).await;
    let now = time::OffsetDateTime::now_utc();
    let scopes = ["vc.issue".to_owned()];
    let old = vc_core::grant::request_grant(
        &pool,
        application,
        GUILD,
        &scopes,
        600,
        now - time::Duration::seconds(600),
    )
    .await
    .unwrap();
    let (first, second) = tokio::join!(
        vc_core::grant::request_grant(&pool, application, GUILD, &scopes, 600, now),
        vc_core::grant::request_grant(&pool, application, GUILD, &scopes, 600, now),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first.device_code, old.device_code);
    assert_eq!(first.device_code, second.device_code);
    assert_eq!(first.user_code, second.user_code);
    assert_eq!(
        vc_core::grant::requests_of(&pool, application)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        vc_core::grant::poll_request(&pool, application, first.device_code, now)
            .await
            .unwrap()
            .is_some()
    );
}
