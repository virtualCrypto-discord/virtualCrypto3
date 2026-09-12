#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request};
use serde_json::{Map, Value};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use tower::ServiceExt;
use uuid::Uuid;
use vc_api::AppState;
use vc_api::discord::{DiscordApi, DiscordError, RefreshedToken};
use vc_auth::claims::{AUDIENCE, Claims, ISSUER};

pub const JWT_SECRET: &str = "test-secret";
pub const REFRESHED_TOKEN: &str = "refreshed-token";
pub const REFRESHED_REFRESH_TOKEN: &str = "refreshed-refresh-token";

pub fn utc_now() -> PrimitiveDateTime {
    let now = OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero is a valid nanosecond");
    PrimitiveDateTime::new(now.date(), now.time())
}

/// Stands in for Discord so tests never touch the network. Returns exactly the
/// payload the Elixir goldens were captured with, and counts refresh calls so
/// tests can prove when the refresh branch is (not) taken.
pub struct FakeDiscord {
    payload: Map<String, Value>,
    refresh_calls: AtomicUsize,
}

impl FakeDiscord {
    pub fn new() -> Self {
        Self {
            payload: golden_discord_payload(),
            refresh_calls: AtomicUsize::new(0),
        }
    }

    pub fn refresh_calls(&self) -> usize {
        self.refresh_calls.load(Ordering::SeqCst)
    }
}

impl Default for FakeDiscord {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DiscordApi for FakeDiscord {
    async fn get_user_info(&self, _token: &str) -> Result<Map<String, Value>, DiscordError> {
        Ok(self.payload.clone())
    }

    async fn refresh_token(&self, _refresh_token: &str) -> Result<RefreshedToken, DiscordError> {
        self.refresh_calls.fetch_add(1, Ordering::SeqCst);

        Ok(RefreshedToken {
            token: REFRESHED_TOKEN.to_string(),
            expires_in: 3600,
            refresh_token: Some(REFRESHED_REFRESH_TOKEN.to_string()),
        })
    }

    /// Discord returns ids as strings; the Elixir test fake happened to echo the
    /// integer it was given, but production always sees a string.
    async fn get_user(
        &self,
        discord_user_id: i64,
    ) -> Result<Option<Map<String, Value>>, DiscordError> {
        let mut user = Map::new();
        user.insert("id".to_string(), Value::String(discord_user_id.to_string()));

        Ok(Some(user))
    }
}

fn golden_discord_payload() -> Map<String, Value> {
    let value = serde_json::json!({
        "id": "100000000000000001",
        "username": "tester",
        "discriminator": "0001",
        "avatar": null,
        "bot": false,
        "system": false,
        "mfa_enabled": true,
        "premium_type": 2,
        "public_flags": 0,
        "extra_field_that_must_be_filtered": "ignored"
    });

    match value {
        Value::Object(map) => map,
        _ => unreachable!("the fixture is an object"),
    }
}

pub fn fake() -> Arc<FakeDiscord> {
    Arc::new(FakeDiscord::new())
}

pub fn state(pool: PgPool, discord: Arc<FakeDiscord>) -> AppState {
    AppState::new(pool, JWT_SECRET, discord)
}

pub async fn insert_user(pool: &PgPool, id: i32, discord_id: i64) {
    let at = utc_now();
    sqlx::query!(
        "INSERT INTO users (id, status, discord_id, inserted_at, updated_at)
         VALUES ($1, 0, $2, $3, $3)",
        id,
        discord_id,
        at
    )
    .execute(pool)
    .await
    .expect("insert user");

    // The row was inserted with an explicit id, so move the sequence past it:
    // production code creates users without an id and would otherwise collide.
    sqlx::query("SELECT setval('users_id_seq', (SELECT MAX(id) FROM users))")
        .execute(pool)
        .await
        .expect("advance users sequence");
}

pub async fn insert_discord_auth(pool: &PgPool, discord_user_id: i64, token: &str) {
    let at = utc_now();
    sqlx::query!(
        "INSERT INTO discord_users (discord_user_id, token, refresh_token, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)",
        discord_user_id,
        token,
        "stub-refresh-token",
        at
    )
    .execute(pool)
    .await
    .expect("insert discord auth");
}

pub async fn set_discord_updated_at(
    pool: &PgPool,
    discord_user_id: i64,
    updated_at: PrimitiveDateTime,
) {
    sqlx::query!(
        "UPDATE discord_users SET updated_at = $1 WHERE discord_user_id = $2",
        updated_at,
        discord_user_id
    )
    .execute(pool)
    .await
    .expect("set updated_at");
}

pub async fn discord_auth_row(
    pool: &PgPool,
    discord_user_id: i64,
) -> (
    Option<String>,
    Option<String>,
    Option<PrimitiveDateTime>,
    PrimitiveDateTime,
) {
    let row = sqlx::query!(
        "SELECT token, refresh_token, expires, updated_at
           FROM discord_users WHERE discord_user_id = $1",
        discord_user_id
    )
    .fetch_one(pool)
    .await
    .expect("read discord auth");

    (row.token, row.refresh_token, row.expires, row.updated_at)
}

/// Mint a token the way Guardian does: sign the claims and record the jti in
/// `user_access_tokens`, which is what makes the token acceptable.
pub async fn mint(pool: &PgPool, subject: i32, scopes: &[&str]) -> String {
    let jti = Uuid::new_v4();
    let at = utc_now();
    let issued = OffsetDateTime::now_utc().unix_timestamp();

    sqlx::query!(
        "INSERT INTO user_access_tokens (user_id, token_id, expires, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)",
        i64::from(subject),
        jti,
        at + Duration::hours(1),
        at
    )
    .execute(pool)
    .await
    .expect("insert user access token");

    let claims = Claims {
        sub: subject.to_string(),
        exp: issued + 3600,
        iat: Some(issued),
        nbf: Some(issued),
        iss: ISSUER.to_string(),
        aud: Some(AUDIENCE.to_string()),
        jti: jti.to_string(),
        kind: "user".to_string(),
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        typ: Some("access".to_string()),
    };

    vc_auth::jwt::sign(&claims, JWT_SECRET.as_bytes()).expect("sign token")
}

/// Delete the jti row, exactly like Guardian.revoke/1.
pub async fn revoke(pool: &PgPool, token: &str) {
    let claims = vc_auth::jwt::verify(token, JWT_SECRET.as_bytes()).expect("verify token");
    let jti = Uuid::parse_str(&claims.jti).expect("jti is a uuid");

    sqlx::query!("DELETE FROM user_access_tokens WHERE token_id = $1", jti)
        .execute(pool)
        .await
        .expect("delete token row");
}

pub struct Golden {
    pub status: u16,
    pub body: Value,
}

pub fn golden(raw: &str) -> Golden {
    let value: Value = serde_json::from_str(raw).expect("golden is valid json");

    Golden {
        status: value["status"].as_u64().expect("golden status") as u16,
        body: value["body"].clone(),
    }
}

pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Value,
}

pub async fn get_with_accept(
    app: Router,
    uri: &str,
    bearer: Option<&str>,
    accept: Option<&str>,
) -> Response {
    let mut builder = Request::builder().method("GET").uri(uri);

    if let Some(accept) = accept {
        builder = builder.header("accept", accept);
    }
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }

    let response = app
        .oneshot(builder.body(Body::empty()).expect("request"))
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

pub async fn get(app: Router, uri: &str, bearer: Option<&str>) -> Response {
    get_with_accept(app, uri, bearer, Some("application/json")).await
}

/// Compares only the parts of the response that are the API contract: status and
/// parsed body. Headers that are incidental to Phoenix (`cache-control`,
/// `x-request-id`, the charset parameter) are deliberately not replicated.
pub fn assert_matches_golden(actual: &Response, expected: &Golden) {
    assert_eq!(actual.status, expected.status, "status");
    assert_eq!(actual.body, expected.body, "body");

    let content_type = actual
        .headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .expect("content-type is set");

    assert!(
        content_type.starts_with("application/json"),
        "expected a JSON content-type, got {content_type}"
    );
}

/// Assert on the parts of a response that are the API contract.
pub fn assert_json(actual: &Response, status: u16, body: Value) {
    assert_eq!(actual.status, status, "status");
    assert_eq!(actual.body, body, "body");
}

/// A currency row with an explicit id and pool so fixtures are deterministic.
pub async fn insert_currency(
    pool: &PgPool,
    id: i64,
    name: &str,
    unit: &str,
    guild_id: i64,
    pool_amount: i64,
) {
    let at = utc_now();
    sqlx::query!(
        "INSERT INTO currencies (id, name, unit, guild_id, pool_amount, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $6)",
        id,
        name,
        unit,
        guild_id,
        pool_amount,
        at
    )
    .execute(pool)
    .await
    .expect("insert currency");

    sqlx::query("SELECT setval('info_id_seq', (SELECT MAX(id) FROM currencies))")
        .execute(pool)
        .await
        .expect("advance currencies sequence");
}

pub async fn insert_asset(pool: &PgPool, user_id: i32, currency_id: i64, amount: i64) {
    let at = utc_now();
    sqlx::query!(
        "INSERT INTO assets (user_id, currency_id, amount, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)",
        i64::from(user_id),
        currency_id,
        amount,
        at
    )
    .execute(pool)
    .await
    .expect("insert asset");
}

/// Fixed so serialized timestamps are deterministic.
pub const CLAIM_AT: PrimitiveDateTime = time::macros::datetime!(2020-01-02 03:04:05);
pub const CLAIM_AT_RFC3339: &str = "2020-01-02T03:04:05Z";

pub async fn insert_claim(
    pool: &PgPool,
    id: i64,
    amount: i64,
    status: &str,
    claimant_user_id: i32,
    payer_user_id: i32,
    currency_id: i64,
) -> i64 {
    let claim_id = sqlx::query_scalar!(
        "INSERT INTO claims (id, amount, status, claimant_user_id, payer_user_id, currency_id,
                             inserted_at, updated_at)
         VALUES ($1, $2, $3::text::virtual_crypto_claim_status, $4, $5, $6, $7, $7)
         RETURNING id",
        id,
        amount,
        status,
        i64::from(claimant_user_id),
        i64::from(payer_user_id),
        currency_id,
        CLAIM_AT
    )
    .fetch_one(pool)
    .await
    .expect("insert claim");

    // The row was inserted with an explicit id, so move the sequence past it:
    // production code creates claims without an id and would otherwise collide.
    sqlx::query("SELECT setval('claims_id_seq', (SELECT MAX(id) FROM claims))")
        .execute(pool)
        .await
        .expect("advance claims sequence");

    claim_id
}

pub async fn insert_claim_metadata(
    pool: &PgPool,
    claim_id: i64,
    claimant_user_id: i32,
    payer_user_id: i32,
    owner_user_id: i32,
    metadata: Value,
) {
    sqlx::query!(
        "INSERT INTO claim_metadata (claim_id, claimant_user_id, payer_user_id, owner_user_id, metadata)
         VALUES ($1, $2, $3, $4, $5)",
        claim_id,
        i64::from(claimant_user_id),
        i64::from(payer_user_id),
        i64::from(owner_user_id),
        metadata
    )
    .execute(pool)
    .await
    .expect("insert claim metadata");
}
