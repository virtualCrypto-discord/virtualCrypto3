//! `/applications/{id}/grants`: the guilds an application may issue in, as the
//! application's own page manages them.
//!
//! Additions rather than ports — the Elixir had no read that could list a grant
//! and no call that could take one away, because a grant only ever appeared as a
//! side effect of redeeming an authorization code. The shape follows
//! `tests/connect.rs` next door: the `client_id` in the path, the caller's own
//! application, and a 404 for one that is not theirs.
//!
//! Read and revoke only, on purpose: writing a grant is the guild's decision,
//! through an ask the application makes and the guild answers in Discord, or
//! through the consent screen. This endpoint lists what the guild decided and
//! takes it back — it never writes one.

mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    Recorded, Response, account_of, client_id_of, fake, get, insert_application, insert_grant,
    insert_user, mint, mint_app, state, state_with_notifier,
};
use tower::ServiceExt;

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const A_CLIENT_ID: &str = "00000000-0000-0000-0000-000000000000";
const GUILD: i64 = 900_000_000_000_000_001;

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
/// nothing holds nothing, which the page reads as "nothing to take back".
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
        vc_api::router(state(pool, fake())),
        &grants_uri(&client_id),
        Some(&token),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body[0]["guild_id"], GUILD.to_string());
    assert_eq!(response.body[0]["scopes"], json!(["vc.issue"]));
}

/// What the guild decided issues: the issuance endpoint's own answer, with the
/// guild token the device poll would hand the application.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_granted_guild_issues(pool: PgPool) {
    let (application, _, _) = fixture(&pool).await;
    support::insert_currency(&pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_grant(&pool, application, GUILD, &["vc.issue"]).await;

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

/// The owner's revoke is a decision, and the application is told about it the
/// way the guild's approval tells it: a device that named a webhook learns the
/// permission is gone without polling for the difference.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_pings_the_application(pool: PgPool) {
    let (application, client_id, token) = fixture(&pool).await;
    insert_grant(&pool, application, GUILD, &["vc.issue"]).await;
    let notified = Arc::new(Recorded::default());

    let response = request(
        vc_api::router(state_with_notifier(pool.clone(), fake(), notified.clone())),
        "DELETE",
        format!("{}/{}", grants_uri(&client_id), GUILD),
        Some(&token),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 204, "body: {}", response.body);
    assert_eq!(
        notified.grant_decisions(),
        [(application, GUILD)],
        "the application, and the guild it may no longer issue in"
    );
}

/// Nothing to take back is nothing to tell: a guild the application holds no
/// grant in is not a decision anyone made.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_what_was_never_granted_pings_nobody(pool: PgPool) {
    let (_, client_id, token) = fixture(&pool).await;
    let notified = Arc::new(Recorded::default());

    let response = request(
        vc_api::router(state_with_notifier(pool.clone(), fake(), notified.clone())),
        "DELETE",
        format!("{}/{}", grants_uri(&client_id), GUILD),
        Some(&token),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 204, "body: {}", response.body);
    assert!(notified.grant_decisions().is_empty(), "nothing was decided");
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
