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
use support::{Response, account_of, fake, insert_application, insert_user, mint, mint_app, state};
use tower::ServiceExt;
use vc_core::grant::Target;

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const GUILD: i64 = 900_000_000_000_000_001;
const PERSON: i64 = 700_000_000_000_000_001;
const SOMEBODY_ELSE: i64 = 700_000_000_000_000_002;

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

    assert_eq!(row.guild_id, Some(GUILD));
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
        Target::Guild(GUILD),
        &["vc.issue".to_owned()],
        &[],
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

    vc_core::grant::decide_request(
        &pool,
        &user_code,
        Target::Guild(GUILD),
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
            "device_code": response.body[0]["device_code"],
            "user_code": user_code,
            "guild_id": GUILD.to_string(),
            "discord_id": Value::Null,
            "scopes": ["vc.issue"],
            "status": "approved",
            "grant_id": response.body[0]["grant_id"],
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
    vc_core::grant::decide_request(
        &pool,
        &user_code,
        Target::Guild(GUILD),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("the answer");

    // After: the guild token.
    let (status, body) = device_poll(&pool, application, &device_code).await;

    assert_eq!(status, 200, "body: {body}");
    assert_eq!(body["token_type"], "Bearer");
    assert!(body.get("refresh_token").is_none(), "refresh is opt-in");

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

async fn refresh_device_token(pool: &PgPool, token: &str) -> (u16, Value) {
    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/oauth2/token")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({
                        "grant_type": "refresh_token", "refresh_token": token
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn device_refresh_preserves_the_approved_grant_after_the_ask_and_access_token_expire(
    pool: PgPool,
) {
    let (application, _) = fixture(&pool).await;
    insert_user(&pool, 101, PERSON).await;
    support::insert_currency(&pool, 1, "nyan", "n", GUILD, 100).await;
    sqlx::query("UPDATE applications SET grant_types = ARRAY['refresh_token']::openid_connect_grant_types[] WHERE id = $1")
        .bind(application).execute(&pool).await.unwrap();

    for (target, scope) in [
        (Target::Guild(GUILD), "vc.issue"),
        (Target::User(PERSON), "vc.pay"),
    ] {
        let now = time::OffsetDateTime::now_utc();
        let asked = vc_core::grant::request_grant(
            &pool,
            application,
            target,
            &[scope.into()],
            &[1],
            600,
            now,
        )
        .await
        .unwrap();
        vc_core::grant::decide_request(&pool, &asked.user_code, target, now)
            .await
            .unwrap()
            .unwrap();
        let (status, issued) =
            device_poll(&pool, application, &asked.device_code.to_string()).await;
        assert_eq!(status, 200, "{issued}");
        let grant_id: i64 = issued["grant_id"].as_str().unwrap().parse().unwrap();
        let refresh = issued["refresh_token"].as_str().unwrap();

        // An expired ask may have been purged by the time the access token dies.
        sqlx::query("DELETE FROM grant_requests WHERE id = $1")
            .bind(asked.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE access_tokens SET expires = now() - interval '2 seconds' WHERE grant_id = $1",
        )
        .bind(grant_id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            vc_core::grant::resolve_token(
                &pool,
                issued["access_token"].as_str().unwrap().parse().unwrap(),
                time::OffsetDateTime::now_utc()
            )
            .await
            .unwrap()
            .is_none()
        );

        let (status, renewed) = refresh_device_token(&pool, refresh).await;
        assert_eq!(status, 200, "{renewed}");
        let access: uuid::Uuid = renewed["access_token"].as_str().unwrap().parse().unwrap();
        let resolved = vc_core::grant::resolve_token(&pool, access, now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.application_id, application);
        assert_eq!(resolved.target, target);
        assert_eq!(resolved.scopes, [scope]);
        assert_eq!(resolved.resources, [1]);
        let renewed_grant: i64 =
            sqlx::query_scalar("SELECT grant_id FROM access_tokens WHERE token_id = $1")
                .bind(access)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(renewed_grant, grant_id);

        // Every replacement is usable once, and revocation ends renewal too.
        let (status, refused) = refresh_device_token(&pool, refresh).await;
        assert_eq!(status, 400, "{refused}");
        let next_refresh = renewed["refresh_token"].as_str().unwrap();
        let (status, again) = refresh_device_token(&pool, next_refresh).await;
        assert_eq!(status, 200, "{again}");
        sqlx::query("DELETE FROM grants WHERE id = $1")
            .bind(grant_id)
            .execute(&pool)
            .await
            .unwrap();
        let (status, refused) =
            refresh_device_token(&pool, again["refresh_token"].as_str().unwrap()).await;
        assert_eq!(status, 400, "{refused}");
        assert_eq!(refused["error_description"], "invalid_refresh_token");
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_failed_device_refresh_token_insert_rolls_back_the_access_token(pool: PgPool) {
    let (application, _) = fixture(&pool).await;
    sqlx::query("UPDATE applications SET grant_types = ARRAY['refresh_token']::openid_connect_grant_types[] WHERE id = $1")
        .bind(application).execute(&pool).await.unwrap();
    let now = time::OffsetDateTime::now_utc();
    let asked = vc_core::grant::request_grant(
        &pool,
        application,
        Target::Guild(GUILD),
        &["vc.issue".into()],
        &[],
        600,
        now,
    )
    .await
    .unwrap();
    vc_core::grant::decide_request(&pool, &asked.user_code, Target::Guild(GUILD), now)
        .await
        .unwrap()
        .unwrap();
    sqlx::raw_sql("CREATE FUNCTION refuse_refresh() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test failure'; END $$;
        CREATE TRIGGER refuse_refresh BEFORE INSERT ON refresh_tokens FOR EACH ROW EXECUTE FUNCTION refuse_refresh();")
        .execute(&pool).await.unwrap();
    assert_eq!(
        device_poll(&pool, application, &asked.device_code.to_string())
            .await
            .0,
        400
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
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
        Target::Guild(GUILD),
        &["vc.issue".to_owned()],
        &[],
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
        vc_core::grant::decide_request(&pool, code, Target::Guild(GUILD), now)
            .await
            .unwrap()
            .map(|decided| decided.application_id),
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
        Target::Guild(GUILD),
        &scopes,
        &[],
        600,
        now - time::Duration::seconds(600),
    )
    .await
    .unwrap();
    let (first, second) = tokio::join!(
        vc_core::grant::request_grant(
            &pool,
            application,
            Target::Guild(GUILD),
            &scopes,
            &[],
            600,
            now
        ),
        vc_core::grant::request_grant(
            &pool,
            application,
            Target::Guild(GUILD),
            &scopes,
            &[],
            600,
            now
        ),
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

async fn poll_after_discord_revocation(pool: PgPool, keep_another: bool) {
    let (application, _) = fixture(&pool).await;
    let scopes = if keep_another {
        vec!["vc.issue".to_owned(), "vc.contract".to_owned()]
    } else {
        vec!["vc.issue".to_owned()]
    };
    let asked = vc_core::grant::request_grant(
        &pool,
        application,
        Target::Guild(GUILD),
        &scopes,
        &[],
        600,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert!(
        vc_core::grant::decide_request(
            &pool,
            &asked.user_code,
            Target::Guild(GUILD),
            time::OffsetDateTime::now_utc()
        )
        .await
        .unwrap()
        .is_some()
    );
    let client_id = support::client_id_of(&pool, application).await;
    if keep_another {
        // The legacy owner endpoint removes only issuing permission in bulk.
        vc_core::grant::revoke_grant(&pool, &client_id, GUILD)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            vc_core::grant::grants_of(&pool, application).await.unwrap()[0].scopes,
            ["vc.contract"]
        );
    } else {
        let grant: i64 = sqlx::query_scalar("SELECT grant_id FROM grant_requests WHERE id = $1")
            .bind(asked.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let revoked = support::interaction(vc_api::router(state(pool.clone(), fake())), json!({
            "type":3, "data":{"component_type":2,"custom_id":vc_api::custom_id::ui::grant::revoke_one_custom_id(grant)},
            "member":{"user":{"id":OWNER_DISCORD_ID.to_string()},"permissions":support::DEFAULT_PERMISSIONS.to_string()},
            "guild_id":GUILD.to_string()
        })).await;
        assert_eq!(revoked.status, 200);
        assert!(
            vc_core::grant::grants_of(&pool, application)
                .await
                .unwrap()
                .is_empty()
        );
    }
    let (status, body) = device_poll(&pool, application, &asked.device_code.to_string()).await;
    assert_eq!(status, 400, "revoked poll returned {body}");
    assert_eq!(body["error"], "invalid_grant");
    let tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(tokens, 0, "a refused poll must not persist a token");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_poll_after_discord_revocation_is_invalid_grant(pool: PgPool) {
    poll_after_discord_revocation(pool, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn another_remaining_scope_does_not_replace_revoked_issue_permission(pool: PgPool) {
    poll_after_discord_revocation(pool, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_repeated_ask_reports_its_remaining_lifetime(pool: PgPool) {
    let (application, token) = fixture(&pool).await;
    let asked = vc_core::grant::request_grant(
        &pool,
        application,
        Target::Guild(GUILD),
        &["vc.issue".to_owned()],
        &[],
        600,
        time::OffsetDateTime::now_utc() - time::Duration::seconds(590),
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
    assert_eq!(response.status, 201);
    assert_eq!(response.body["device_code"], asked.device_code.to_string());
    assert_eq!(response.body["user_code"], asked.user_code);
    assert!(
        vc_core::grant::poll_request(
            &pool,
            application,
            asked.device_code,
            time::OffsetDateTime::now_utc() + time::Duration::seconds(11)
        )
        .await
        .unwrap()
        .is_none()
    );
    let expires_in = response.body["expires_in"].as_i64().unwrap();
    assert!(
        (1..=10).contains(&expires_in),
        "only 10 seconds remain, but returned {expires_in}"
    );
}

/// A person's ask: their code is theirs. What answers it is being them, and
/// nothing else — not the guild the code was not put to, and not another person
/// who happens to have read it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_person_is_asked_and_only_their_code_answers_it(pool: PgPool) {
    let application = insert_application(&pool, OWNER_DISCORD_ID, "an application").await;
    let owner = account_of(&pool, application).await;
    let token = mint_app(&pool, owner, &["oauth2.register"]).await;
    insert_user(&pool, 2, PERSON).await;
    insert_user(&pool, 3, SOMEBODY_ELSE).await;

    let asked = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "discord_id": PERSON.to_string(), "scopes": ["vc.delegate.balances.read", "vc.delegate.payments.create"] }),
    )
    .await;
    assert_eq!(asked.status, 201, "{}", asked.body);

    let code = asked
        .body
        .get("user_code")
        .and_then(Value::as_str)
        .expect("a user code")
        .to_owned();
    let now = time::OffsetDateTime::now_utc();

    // The guild the ask was not put to cannot answer it, and neither can another
    // person: a code names one target, and being that target is the whole of the
    // question.
    for target in [Target::Guild(GUILD), Target::User(SOMEBODY_ELSE)] {
        assert_eq!(
            vc_core::grant::decide_request(&pool, &code, target, now)
                .await
                .unwrap(),
            None,
            "{target:?} answered a code that is not theirs"
        );
    }

    assert_eq!(
        vc_core::grant::decide_request(&pool, &code, Target::User(PERSON), now)
            .await
            .unwrap()
            .map(|decided| decided.application_id),
        Some(application)
    );

    // What the grant became: written for the person, carrying what they agreed to,
    // and its token acts as *them* rather than as the application that asked.
    let asked = vc_core::grant::requests_of(&pool, application)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(asked.target, Target::User(PERSON));
    assert_eq!(asked.status, "approved");

    let minted = vc_core::grant::create_device_token(&pool, &asked, now)
        .await
        .unwrap()
        .expect("the approved ask hands out a token")
        .access_token;
    let resolved =
        vc_core::grant::resolve_token(&pool, uuid::Uuid::parse_str(&minted).unwrap(), now)
            .await
            .unwrap()
            .expect("the token resolves");

    assert_eq!(resolved.application_id, application);
    assert_eq!(resolved.target, Target::User(PERSON));
    assert_eq!(
        resolved.account_id, 2,
        "the account is the person's, not the application's"
    );
    assert_eq!(
        resolved.scopes,
        ["vc.delegate.balances.read", "vc.delegate.payments.create"]
    );

    // And it is not a guild token: the guild extractor goes on the target, so the
    // one place a guild's token is accepted refuses this one.
    let refused = tower::ServiceExt::oneshot(
        vc_api::router(state(pool.clone(), fake())),
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/v2/currencies/issue")
            .header("authorization", format!("Bearer {minted}"))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({ "unit": "nyan", "amount": "1" }).to_string(),
            ))
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(refused.status(), 401);

    // The application's own view of its asks names the person, and the guild it
    // was not put to is a null rather than an absence.
    let listed = request(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert_eq!(listed.body[0]["discord_id"], PERSON.to_string());
    assert_eq!(listed.body[0]["guild_id"], Value::Null);
}

/// The two scope sets do not overlap: what a guild can hand over is its pool and
/// what a person can is their own account, so an ask that names the other's scope
/// is refused rather than approved into a grant nobody could use.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_person_is_not_asked_for_a_guilds_scope(pool: PgPool) {
    let application = insert_application(&pool, OWNER_DISCORD_ID, "an application").await;
    let token = mint_app(
        &pool,
        account_of(&pool, application).await,
        &["oauth2.register"],
    )
    .await;

    let refused = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "discord_id": PERSON.to_string(), "scopes": ["vc.issue"] }),
    )
    .await;
    assert_eq!(refused.status, 400);
    assert_eq!(refused.body["error"], "invalid_scope");

    let refused = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        Some(&token),
        json!({ "guild_id": GUILD.to_string(), "scopes": ["vc.pay"] }),
    )
    .await;
    assert_eq!(refused.status, 400);
    assert_eq!(refused.body["error"], "invalid_scope");

    // And a request is put to one of the two: a body that names both is waiting on
    // two different people's answers, and one that names neither is waiting on
    // nobody's.
    for body in [
        json!({ "guild_id": GUILD.to_string(), "discord_id": PERSON.to_string(), "scopes": ["vc.issue"] }),
        json!({ "scopes": ["vc.issue"] }),
    ] {
        let refused = request(
            vc_api::router(state(pool.clone(), fake())),
            "POST",
            Some(&token),
            body.clone(),
        )
        .await;
        assert_eq!(refused.status, 400, "{body}");
        assert_eq!(refused.body["error"], "invalid_request");
    }
}
