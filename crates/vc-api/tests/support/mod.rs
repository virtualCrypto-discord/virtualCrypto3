#![allow(dead_code)]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request};
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use time::{OffsetDateTime, PrimitiveDateTime};
use tower::ServiceExt;
use uuid::Uuid;
use vc_api::AppState;
use vc_api::discord::{DiscordApi, DiscordError, RefreshedToken};
use vc_api::rate_limit::RateLimiter;
use vc_api::state::Links;
use vc_core::claim::Transition;
use vc_core::notification::{NoopNotifier, Notifier};

pub const JWT_SECRET: &str = "test-secret";
/// The session cookie's own secret, which a test needs to read one back.
pub const SESSION_SECRET: &str = "test-session-secret";
pub const REFRESHED_TOKEN: &str = "refreshed-token";
pub const REFRESHED_REFRESH_TOKEN: &str = "refreshed-refresh-token";
/// Distinct from the refresh pair so a test can tell which path ran.
pub const EXCHANGED_TOKEN: &str = "exchanged-token";
pub const EXCHANGED_REFRESH_TOKEN: &str = "exchanged-refresh-token";

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
    guild: Map<String, Value>,
    refresh_calls: AtomicUsize,
    user_calls: AtomicUsize,
    webhooks: Mutex<Vec<Value>>,
}

impl FakeDiscord {
    pub fn new() -> Self {
        Self::with_guild(json!({ "name": "TestGuild" }))
    }

    /// The guild `get_guild` reports. The interaction tests vary it to cover the
    /// icon branches; the id always echoes the request, like the Elixir fakes.
    pub fn with_guild(guild: Value) -> Self {
        let Value::Object(guild) = guild else {
            panic!("the fake guild must be an object");
        };

        Self {
            payload: golden_discord_payload(),
            guild,
            refresh_calls: AtomicUsize::new(0),
            user_calls: AtomicUsize::new(0),
            webhooks: Mutex::new(Vec::new()),
        }
    }

    pub fn refresh_calls(&self) -> usize {
        self.refresh_calls.load(Ordering::SeqCst)
    }

    /// How many times `get_user` reached this stand-in, which is how a cache is
    /// shown to have answered instead.
    pub fn user_calls(&self) -> usize {
        self.user_calls.load(Ordering::SeqCst)
    }

    /// The bodies `post_webhook_message` has been handed, in order — what the
    /// Elixir tests read out of their process mailbox.
    pub fn webhooks(&self) -> Vec<Value> {
        self.webhooks
            .lock()
            .expect("the fake is not poisoned")
            .clone()
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

    async fn get_guild(&self, guild_id: i64) -> Result<Option<Map<String, Value>>, DiscordError> {
        let mut guild = self.guild.clone();
        guild.insert("id".to_string(), Value::String(guild_id.to_string()));

        Ok(Some(guild))
    }

    async fn post_webhook_message(
        &self,
        _application_id: &str,
        _token: &str,
        body: &Value,
    ) -> Result<(), DiscordError> {
        self.webhooks
            .lock()
            .expect("the fake is not poisoned")
            .push(body.clone());

        Ok(())
    }

    async fn refresh_token(&self, _refresh_token: &str) -> Result<RefreshedToken, DiscordError> {
        self.refresh_calls.fetch_add(1, Ordering::SeqCst);

        Ok(RefreshedToken {
            token: REFRESHED_TOKEN.to_string(),
            expires_in: 3600,
            refresh_token: Some(REFRESHED_REFRESH_TOKEN.to_string()),
        })
    }

    async fn exchange_code(&self, _code: &str) -> Result<RefreshedToken, DiscordError> {
        Ok(RefreshedToken {
            token: EXCHANGED_TOKEN.to_string(),
            expires_in: 3600,
            refresh_token: Some(EXCHANGED_REFRESH_TOKEN.to_string()),
        })
    }

    /// Recognizable rather than real, so a test can assert a browser was sent to
    /// Discord and to which state.
    fn authorize_url(&self, state: &str) -> String {
        format!("https://discord.test/authorize?state={state}")
    }

    /// Discord returns ids as strings; the Elixir test fake happened to echo the
    /// integer it was given, but production always sees a string.
    async fn get_user(
        &self,
        discord_user_id: i64,
    ) -> Result<Option<Map<String, Value>>, DiscordError> {
        self.user_calls.fetch_add(1, Ordering::SeqCst);

        // Give a second caller the chance to reach the cache while this one is
        // still out, so a lookup that is not coalesced has somewhere to show it.
        tokio::task::yield_now().await;

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

/// A fake Discord whose `get_guild` reports `guild`.
pub fn fake_with_guild(guild: Value) -> Arc<FakeDiscord> {
    Arc::new(FakeDiscord::with_guild(guild))
}

pub fn state(pool: PgPool, discord: Arc<FakeDiscord>) -> AppState {
    state_with_notifier(pool, discord, Arc::new(NoopNotifier))
}

/// A state whose requests are held to `limiter`, which is how the rate limit is
/// exercised without making a hundred requests.
pub fn state_with_limiter(
    pool: PgPool,
    discord: Arc<FakeDiscord>,
    limiter: Arc<RateLimiter>,
) -> AppState {
    AppState::new(
        pool,
        vc_api::state::Signing::new(JWT_SECRET, SESSION_SECRET, false),
        discord_public_key(),
        links(),
        discord,
        Arc::new(NoopNotifier),
        limiter,
    )
}

/// A state whose claim transitions report to `notifier`, which is how the
/// notification tests watch what would be delivered.
pub fn state_with_notifier(
    pool: PgPool,
    discord: Arc<FakeDiscord>,
    notifier: Arc<dyn Notifier>,
) -> AppState {
    AppState::new(
        pool,
        vc_api::state::Signing::new(JWT_SECRET, SESSION_SECRET, false),
        discord_public_key(),
        links(),
        discord,
        notifier,
        // Unlimited by default, so the other tests are not held to it.
        Arc::new(RateLimiter::new(0, vc_api::rate_limit::DEFAULT_WINDOW)),
    )
}

/// The URLs from `config/test.exs`, which the help and invite responses embed
/// verbatim.
pub fn links() -> Links {
    Links {
        site_url: "https://vcrypto.sumidora.com".to_string(),
        invite_url: "https://discord.com/api/oauth2/authorize?client_id=791984306632654869\
                     &permissions=0&scope=applications.commands%20bot"
            .to_string(),
        support_guild_invite_url: "https://discord.com/invite/Hgp5DpG".to_string(),
    }
}

/// The Ed25519 seed the Elixir test config uses for the Discord interaction
/// handshake. The public key is derived from it, so the pair cannot drift.
pub const DISCORD_KEY_SEED: [u8; 32] = [
    39, 17, 61, 144, 80, 58, 130, 10, 180, 113, 133, 86, 163, 239, 126, 99, 222, 218, 21, 76, 55,
    75, 56, 158, 183, 252, 253, 147, 84, 164, 94, 253,
];

pub fn discord_signing_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&DISCORD_KEY_SEED)
}

pub fn discord_public_key() -> [u8; 32] {
    discord_signing_key().verifying_key().to_bytes()
}

/// Sign an interaction body the way Discord does: Ed25519 over
/// `timestamp <> body`, with the signature hex-encoded.
pub fn sign_interaction(body: &[u8]) -> (String, String) {
    let timestamp = OffsetDateTime::now_utc().unix_timestamp().to_string();

    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);

    let signature = ed25519_dalek::Signer::sign(&discord_signing_key(), &message);

    (timestamp, hex::encode(signature.to_bytes()))
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

/// Mint a token through the service's own issuance, so the suite exercises what
/// production runs rather than a parallel copy of it.
pub async fn mint(pool: &PgPool, subject: i32, scopes: &[&str]) -> String {
    vc_auth::issue::user_token(
        pool,
        JWT_SECRET.as_bytes(),
        i64::from(subject),
        scopes,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("issue a token")
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

/// The guild and permissions `InteractionsControllerTest.Helper.Common` uses
/// when a test does not pass its own.
pub const DEFAULT_GUILD: i64 = 494_780_225_280_802_817;
pub const DEFAULT_PERMISSIONS: &str = "18446744073709551615";

/// `Helper.Common.execute_from_guild/2`.
pub fn execute_from_guild(data: Value, user: i64) -> Value {
    from_guild(data, user, DEFAULT_GUILD, DEFAULT_PERMISSIONS)
}

/// `Helper.Common.execute_from_guild/4`, with the guild and permissions given.
pub fn from_guild(data: Value, user: i64, guild_id: i64, permissions: &str) -> Value {
    json!({
        "type": 2,
        "data": data,
        "member": {
            "user": { "id": user.to_string() },
            "permissions": permissions,
        },
        "guild_id": guild_id.to_string(),
    })
}

/// `Helper.Common.execute_from_dm/2`.
pub fn execute_from_dm(data: Value, user: i64) -> Value {
    json!({
        "type": 2,
        "data": data,
        "user": { "id": user.to_string() },
    })
}

/// `Helper.Common.button_from_guild/2`.
pub fn button_from_guild(data: Value, user: i64) -> Value {
    component_from_guild(data, user, 2)
}

/// `Helper.Common.select_from_guild/2`.
pub fn select_from_guild(data: Value, user: i64) -> Value {
    component_from_guild(data, user, 3)
}

fn component_from_guild(data: Value, user: i64, component_type: i64) -> Value {
    let Value::Object(mut data) = data else {
        panic!("component data must be an object");
    };
    data.insert("component_type".to_string(), json!(component_type));

    json!({
        "type": 3,
        "data": Value::Object(data),
        "member": {
            "user": { "id": user.to_string() },
            "permissions": DEFAULT_PERMISSIONS,
        },
        "guild_id": DEFAULT_GUILD.to_string(),
    })
}

/// `Helper.Common.modal_submit_from_guild/2`.
pub fn modal_submit_from_guild(data: Value, user: i64) -> Value {
    json!({
        "type": 5,
        "data": data,
        "member": {
            "user": { "id": user.to_string() },
            "permissions": DEFAULT_PERMISSIONS,
        },
        "guild_id": DEFAULT_GUILD.to_string(),
    })
}

/// `InteractionsCase.execute_interaction/2`: sign the body and POST it.
pub async fn interaction(app: Router, payload: Value) -> Response {
    let body = serde_json::to_vec(&payload).expect("encode body");
    let (timestamp, signature) = sign_interaction(&body);

    let request = Request::builder()
        .method("POST")
        .uri("/api/integrations/discord/interactions")
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .header("x-signature-timestamp", timestamp)
        .header("x-signature-ed25519", signature)
        .body(Body::from(body))
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
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };

    Response {
        status,
        headers,
        body,
    }
}

/// The accounts and balances `EnvironmentBootstrapper.setup_money/1` leaves
/// behind.
///
/// Elixir derives its guilds, users and units from `System.unique_integer/1`;
/// the tests only ever read them back out of the context, so fixed values keep a
/// failure readable.
pub struct Money {
    pub user1: i64,
    pub user2: i64,
    pub guild: i64,
    pub guild2: i64,
    pub name: String,
    pub name2: String,
    pub unit: String,
    pub unit2: String,
    pub currency: i64,
    pub currency2: i64,
}

pub const MONEY_USER1: i64 = 100_000_000_000_000_001;
pub const MONEY_USER2: i64 = 100_000_000_000_000_002;
/// The guilds `setup_money/1` puts its currencies in. They are deliberately not
/// [`DEFAULT_GUILD`], which is what lets a `create` test create a currency in
/// the guild the interaction builders use.
pub const MONEY_GUILD: i64 = 494_780_225_280_802_819;
pub const MONEY_GUILD2: i64 = 494_780_225_280_802_820;

/// `setup_money/1`, as the rows it ends up with: each user created a currency
/// worth 200000 in their own guild, the first user paid the second 500, and the
/// first guild's pool gave the second user 500 more.
pub async fn setup_money(pool: &PgPool) -> Money {
    let name = "nyan".to_string();
    let name2 = "wan".to_string();
    let unit = "n".to_string();
    let unit2 = "w".to_string();

    insert_user(pool, 1, MONEY_USER1).await;
    insert_user(pool, 2, MONEY_USER2).await;
    insert_currency(pool, 1, &name, &unit, MONEY_GUILD, 500).await;
    insert_currency(pool, 2, &name2, &unit2, MONEY_GUILD2, 1000).await;
    insert_asset(pool, 1, 1, 199_500).await;
    insert_asset(pool, 2, 1, 1_000).await;
    insert_asset(pool, 2, 2, 200_000).await;

    Money {
        user1: MONEY_USER1,
        user2: MONEY_USER2,
        guild: MONEY_GUILD,
        guild2: MONEY_GUILD2,
        name,
        name2,
        unit,
        unit2,
        currency: 1,
        currency2: 2,
    }
}

/// The claims `EnvironmentBootstrapper.setup_claim/1` leaves behind.
pub struct ClaimSet {
    pub money: Money,
    /// c1 to c6, in the order the Elixir helper creates them.
    pub ids: Vec<i64>,
}

impl ClaimSet {
    pub fn id(&self, index: usize) -> i64 {
        self.ids[index]
    }
}

/// `setup_claim/1`, built through the real claim paths so the balances reflect
/// the transitions it performs: c3 is approved and moves 500 from user2 to user1,
/// c4 is denied, c5 is cancelled, and c6 is a claim on the claimant themselves.
pub async fn setup_claim(pool: &PgPool) -> ClaimSet {
    let money = setup_money(pool).await;
    let unit = money.unit.clone();
    let mut ids = Vec::new();

    ids.push(create_claim(pool, 1, MONEY_USER2, &unit, 500).await);
    ids.push(create_claim(pool, 2, MONEY_USER1, &unit, 9_999_999).await);

    let approved = create_claim(pool, 1, MONEY_USER2, &unit, 500).await;
    transition_claim(pool, 2, approved, Transition::Approved).await;
    ids.push(approved);

    let denied = create_claim(pool, 1, MONEY_USER2, &unit, 500).await;
    transition_claim(pool, 2, denied, Transition::Denied).await;
    ids.push(denied);

    let canceled = create_claim(pool, 1, MONEY_USER2, &unit, 500).await;
    transition_claim(pool, 1, canceled, Transition::Canceled).await;
    ids.push(canceled);

    ids.push(create_claim(pool, 1, MONEY_USER1, &unit, 100).await);

    ClaimSet { money, ids }
}

async fn create_claim(
    pool: &PgPool,
    claimant_id: i32,
    payer_discord_id: i64,
    unit: &str,
    amount: i64,
) -> i64 {
    vc_core::claim::create(pool, claimant_id, payer_discord_id, unit, amount, None)
        .await
        .expect("create claim")
}

async fn transition_claim(pool: &PgPool, operator_id: i32, claim_id: i64, transition: Transition) {
    vc_core::claim::transition(
        pool,
        &NoopNotifier,
        operator_id,
        claim_id,
        transition,
        Some(Value::Object(Map::new())),
    )
    .await
    .expect("transition the claim");
}

/// `ConditionChecker.get_amount/2`: the balance, or zero when there is no row.
pub async fn get_amount(pool: &PgPool, discord_user_id: i64, currency_id: i64) -> i64 {
    let amount = sqlx::query_scalar!(
        "SELECT assets.amount
           FROM assets
           JOIN users ON users.id = assets.user_id
          WHERE users.discord_id = $1 AND assets.currency_id = $2",
        discord_user_id,
        currency_id
    )
    .fetch_optional(pool)
    .await
    .expect("read balance");

    amount.flatten().unwrap_or(0)
}

/// Move a currency's creation time, which is how the deletion window is reached:
/// Elixir injects `:test_delete_now` through the process dictionary instead.
pub async fn set_currency_inserted_at(
    pool: &PgPool,
    currency_id: i64,
    inserted_at: PrimitiveDateTime,
) {
    sqlx::query!(
        "UPDATE currencies SET inserted_at = $1 WHERE id = $2",
        inserted_at,
        currency_id
    )
    .execute(pool)
    .await
    .expect("age currency");
}

pub struct CurrencyRow {
    pub id: i64,
    pub name: Option<String>,
    pub pool_amount: Option<i64>,
}

/// The currency with this unit, which is how a `create` test finds the row it
/// just made.
pub async fn currency_by_unit(pool: &PgPool, unit: &str) -> Option<CurrencyRow> {
    let row = sqlx::query!(
        "SELECT id, name, pool_amount FROM currencies WHERE unit = $1",
        unit
    )
    .fetch_optional(pool)
    .await
    .expect("read currency");

    row.map(|row| CurrencyRow {
        id: row.id,
        name: row.name,
        pool_amount: row.pool_amount,
    })
}
