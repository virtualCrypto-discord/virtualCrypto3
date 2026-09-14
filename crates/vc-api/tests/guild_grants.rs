//! `/applications/{id}/grants`: the guilds an application may issue in, as the
//! application's own page manages them.
//!
//! Additions rather than ports — the Elixir had no read that could list a grant
//! and no call that could take one away, because a grant only ever appeared as a
//! side effect of redeeming an authorization code. The shape follows
//! `tests/connect.rs` next door: the `client_id` in the path, the caller's own
//! application, and a 404 for one that is not theirs.

mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    FakeDiscord, Response, account_of, client_id_of, fake, get, insert_application, insert_grant,
    insert_user, mint, mint_app, state,
};
use tower::ServiceExt;

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const STRANGER_DISCORD_ID: i64 = 500_000_000_000_000_003;
const A_CLIENT_ID: &str = "00000000-0000-0000-0000-000000000000";
const GUILD: i64 = 900_000_000_000_000_001;

/// The owner of a guild whose administrator bit they carry: `with_member` reads
/// the owner out of the guild, and the role grants the bit `may_act_for_guild`
/// asks for.
fn administrator() -> Arc<FakeDiscord> {
    FakeDiscord::with_member(OWNER_DISCORD_ID, &["7"], &[(7, 0x8)])
}

/// An administrator in name only: the guild knows a member, but this account is
/// neither the owner nor an administrator of it.
fn stranger() -> Arc<FakeDiscord> {
    FakeDiscord::with_member(STRANGER_DISCORD_ID, &["7"], &[(7, 0x1)])
}

async fn request(
    app: axum::Router,
    method: &str,
    uri: String,
    token: Option<&str>,
    body: Value,
) -> Response {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
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

async fn fixture(pool: &PgPool) -> (i64, String, String) {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(pool, OWNER_DISCORD_ID, "mine").await;
    let client_id = client_id_of(pool, application).await;
    let token = mint(pool, OWNER, &["oauth2.register"]).await;

    (application, client_id, token)
}

fn grants_uri(client_id: &str) -> String {
    format!("/applications/{client_id}/grants")
}

/// An empty list rather than nothing: an application that has been granted
/// nothing holds nothing, which the page reads as "add one".
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_with_no_grants_answers_an_empty_list(pool: PgPool) {
    let (_, client_id, token) = fixture(&pool).await;

    let response = get(
        vc_api::router(state(pool, fake())),
        &grants_uri(&client_id),
        Some(&token),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, json!([]));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_names_the_guilds_and_their_scopes(pool: PgPool) {
    let (application, client_id, token) = fixture(&pool).await;
    insert_grant(&pool, application, GUILD, &["vc.issue"]).await;

    let response = get(
        vc_api::router(state(
            pool,
            FakeDiscord::with_member(OWNER_DISCORD_ID, &[], &[]),
        )),
        &grants_uri(&client_id),
        Some(&token),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!([{
            "guild_id": GUILD.to_string(),
            "guild_name": "TestGuild",
            "scopes": ["vc.issue"],
            "updated_at": response.body[0]["updated_at"],
        }])
    );
}

/// The guild's own permission, written by its administrator — this is what
/// `describe` asks the consent screen for, asked by the application's own page.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_administrator_may_allow_their_guild(pool: PgPool) {
    let (application, client_id, token) = fixture(&pool).await;

    let response = request(
        vc_api::router(state(pool.clone(), administrator())),
        "POST",
        grants_uri(&client_id),
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(
        response.body,
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] })
    );

    let granted = sqlx::query!(
        r#"SELECT count(*) AS "count!" FROM grant_scopes s
             JOIN grants g ON g.id = s.grant_id
            WHERE g.application_id = $1 AND g.guild_id = $2
              AND s.scope = 'vc.issue'::virtual_crypto_scope_type"#,
        application,
        GUILD
    )
    .fetch_one(&pool)
    .await
    .expect("the grant");

    assert_eq!(granted.count, 1);
}

/// What was allowed issues: the issuance endpoint's own answer, with the guild
/// token the code flow would hand the application.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_allowed_guild_issues(pool: PgPool) {
    let (application, client_id, token) = fixture(&pool).await;
    support::insert_currency(&pool, 1, "nyan", "nyan", GUILD, 500).await;

    let allowed = request(
        vc_api::router(state(pool.clone(), administrator())),
        "POST",
        grants_uri(&client_id),
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    assert_eq!(allowed.status, 201, "body: {}", allowed.body);

    let guild_token = support::mint_guild_token(&pool, application, GUILD).await;

    let issued = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/currencies/issue".to_string(),
        Some(&guild_token),
        json!({ "receiver_discord_id": OWNER_DISCORD_ID.to_string(), "amount": "100" }),
    )
    .await;

    assert_eq!(issued.status, 201, "body: {}", issued.body);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn somebody_who_may_not_act_for_the_guild_is_refused(pool: PgPool) {
    let (_, client_id, token) = fixture(&pool).await;

    let response = request(
        vc_api::router(state(pool, stranger())),
        "POST",
        grants_uri(&client_id),
        Some(&token),
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    assert_eq!(response.status, 403, "body: {}", response.body);
    assert_eq!(response.body["error"], "forbidden");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_nobody_can_read_is_not_found(pool: PgPool) {
    let (_, client_id, token) = fixture(&pool).await;

    let response = request(
        vc_api::router(state(pool, fake())),
        "POST",
        grants_uri(&client_id),
        Some(&token),
        // The plain fake answers an empty member, and no owner can be read: the
        // guild is one nothing can be asked about, which is `Unknown`.
        json!({ "guild_id": GUILD.to_string() }),
    )
    .await;

    assert_eq!(response.status, 404, "body: {}", response.body);
    assert_eq!(response.body["error"], "not_found");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_id_that_is_not_a_snowflake_is_400(pool: PgPool) {
    let (_, client_id, token) = fixture(&pool).await;

    let response = request(
        vc_api::router(state(pool, fake())),
        "POST",
        grants_uri(&client_id),
        Some(&token),
        json!({ "guild_id": "not-a-snowflake" }),
    )
    .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(response.body["error"], "invalid_request");
}

/// Taking the permission back keeps the grant and drops the scope, which is also
/// what stops an already-issued guild token from issuing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_takes_the_scope_and_leaves_the_grant(pool: PgPool) {
    let (application, client_id, token) = fixture(&pool).await;
    let guild_token = insert_grant(&pool, application, GUILD, &["vc.issue"]).await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "DELETE",
        format!("{}/{}", grants_uri(&client_id), GUILD),
        Some(&token),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 204, "body: {}", response.body);

    let scopes = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM grant_scopes s
           JOIN grants g ON g.id = s.grant_id
          WHERE g.application_id = $1 AND g.guild_id = $2",
        application,
        GUILD
    )
    .fetch_one(&pool)
    .await
    .expect("the scopes");

    assert_eq!(scopes, 0);

    let issued = request(
        vc_api::router(state(pool, fake())),
        "POST",
        "/api/v2/currencies/issue".to_string(),
        Some(&guild_token),
        json!({ "receiver_discord_id": OWNER_DISCORD_ID.to_string(), "amount": "100" }),
    )
    .await;

    assert_eq!(issued.status, 403, "body: {}", issued.body);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn somebody_elses_application_is_404(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(&pool, 2, 600_000_000_000_000_001).await;
    insert_application(&pool, 600_000_000_000_000_001, "theirs").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = request(
        vc_api::router(state(pool, fake())),
        "GET",
        grants_uri(A_CLIENT_ID),
        Some(&token),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 404, "body: {}", response.body);
    assert_eq!(response.body["error"], "not_found");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_token_is_refused(pool: PgPool) {
    let (application, client_id, _) = fixture(&pool).await;
    let account = account_of(&pool, application).await;
    let token = mint_app(&pool, account, &["oauth2.register"]).await;

    let response = request(
        vc_api::router(state(pool, fake())),
        "GET",
        grants_uri(&client_id),
        Some(&token),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 401, "body: {}", response.body);
    assert_eq!(response.body["error"], "invalid_kind");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_user_without_the_scope_is_refused(pool: PgPool) {
    let (_, client_id, _) = fixture(&pool).await;
    let token = mint(&pool, OWNER, &["vc.pay"]).await;

    let response = request(
        vc_api::router(state(pool, fake())),
        "GET",
        grants_uri(&client_id),
        Some(&token),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 403, "body: {}", response.body);
    assert_eq!(response.body["error"], "insufficient_scope");
}
