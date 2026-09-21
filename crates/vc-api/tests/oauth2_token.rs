//! The tokens a grant owns.
//!
//! Additions rather than ports: OAuth2 is the part of the migration with no
//! Elixir test to follow.

mod support;

use sqlx::PgPool;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;
use vc_core::application::{authorize, take_code, webhook_data};
use vc_core::grant::{
    EXPIRES_IN, ExchangeError, REFRESH_TOKEN_TTL, create_access_token, create_grant_scopes,
    create_refresh_token, exchange_code, exchange_refresh_token, grant_for_code,
    replace_refresh_token, revoke_access_token, revoke_refresh_token,
};

async fn application(pool: &PgPool) -> i64 {
    sqlx::query_scalar!(
        r#"INSERT INTO applications
             (status, client_id, client_secret, grant_types, client_name,
              public_key, private_key, inserted_at, updated_at)
           VALUES (0, $1, 'secret',
                   ARRAY['authorization_code']::openid_connect_grant_types[],
                   'An Application', '\x00'::bytea, '\x00'::bytea,
                   now()::timestamp(0), now()::timestamp(0))
        RETURNING id"#,
        Uuid::new_v4()
    )
    .fetch_one(pool)
    .await
    .expect("insert the application")
}

/// An application, and a grant of it in a guild.
async fn grant(pool: &PgPool) -> i64 {
    let application_id = application(pool).await;

    sqlx::query_scalar!(
        "INSERT INTO grants (application_id, guild_id, inserted_at, updated_at)
         VALUES ($1, 42, now()::timestamp(0), now()::timestamp(0))
        RETURNING id",
        application_id
    )
    .fetch_one(pool)
    .await
    .expect("insert the grant")
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_is_a_row_that_lasts_an_hour(pool: PgPool) {
    let grant_id = grant(&pool).await;
    let now = OffsetDateTime::now_utc();

    let token = create_access_token(&pool, grant_id, now)
        .await
        .expect("a token");

    let stored = sqlx::query!(
        "SELECT grant_id, expires, inserted_at FROM access_tokens WHERE token_id = $1",
        Uuid::parse_str(&token).expect("the token is a uuid")
    )
    .fetch_one(&pool)
    .await
    .expect("the row");

    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    assert_eq!(stored.grant_id, Some(grant_id));
    assert_eq!(stored.inserted_at, at);
    assert_eq!(stored.expires, Some(at + Duration::hours(1)));
}

/// The token is the row's own id, which is what makes revoking one a delete.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn two_tokens_are_not_the_same_token(pool: PgPool) {
    let grant_id = grant(&pool).await;
    let now = OffsetDateTime::now_utc();

    let one = create_access_token(&pool, grant_id, now)
        .await
        .expect("a token");
    let two = create_access_token(&pool, grant_id, now)
        .await
        .expect("another token");

    assert_ne!(one, two);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_code_makes_a_grant_that_remembers_it(pool: PgPool) {
    let application_id = application(&pool).await;
    let now = OffsetDateTime::now_utc();

    let grant_id = grant_for_code(&pool, application_id, 42, "a-code", now)
        .await
        .expect("a grant")
        .expect("a new one");

    let stored = sqlx::query!(
        "SELECT application_id, guild_id, latest_code FROM grants WHERE id = $1",
        grant_id
    )
    .fetch_one(&pool)
    .await
    .expect("the row");

    assert_eq!(stored.application_id, Some(application_id));
    assert_eq!(stored.guild_id, Some(42));
    assert_eq!(stored.latest_code.as_deref(), Some("a-code"));
}

/// The delete in front of the insert is the check, not housekeeping: a grant
/// that still remembers this code has already been redeemed with it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_code_a_grant_remembers_is_a_reuse(pool: PgPool) {
    let application_id = application(&pool).await;
    let now = OffsetDateTime::now_utc();

    grant_for_code(&pool, application_id, 42, "a-code", now)
        .await
        .expect("a grant");

    let again = grant_for_code(&pool, application_id, 42, "a-code", now)
        .await
        .expect("an answer");

    assert_eq!(again, None);

    let stored = sqlx::query!("SELECT COUNT(*) AS count FROM grants")
        .fetch_one(&pool)
        .await
        .expect("count the grants");

    assert_eq!(stored.count, Some(0), "and the grant it found was deleted");
}

/// Two codes for the same application and guild are one grant, not two.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_second_code_keeps_the_same_grant(pool: PgPool) {
    let application_id = application(&pool).await;
    let now = OffsetDateTime::now_utc();

    let first = grant_for_code(&pool, application_id, 42, "one", now)
        .await
        .expect("a grant")
        .expect("a new one");
    let second = grant_for_code(&pool, application_id, 42, "two", now)
        .await
        .expect("a grant")
        .expect("the same one");

    assert_eq!(first, second);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn scopes_are_recorded_once_however_often_they_are_given(pool: PgPool) {
    let application_id = application(&pool).await;
    let now = OffsetDateTime::now_utc();

    let grant_id = grant_for_code(&pool, application_id, 42, "a-code", now)
        .await
        .expect("a grant")
        .expect("a new one");

    let mut connection = pool.acquire().await.expect("a connection");

    create_grant_scopes(&mut connection, grant_id, &["openid".to_string()], now)
        .await
        .expect("the scopes");
    create_grant_scopes(&mut connection, grant_id, &["openid".to_string()], now)
        .await
        .expect("the same scopes again");

    let stored = sqlx::query!(
        r#"SELECT scope::text AS "scope!" FROM grant_scopes WHERE grant_id = $1"#,
        grant_id
    )
    .fetch_all(&pool)
    .await
    .expect("the rows");

    assert_eq!(stored.len(), 1, "given twice, recorded once");
    assert_eq!(stored[0].scope, "openid");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_grant_has_one_refresh_token_thats_months_long(pool: PgPool) {
    let grant_id = grant(&pool).await;
    let now = OffsetDateTime::now_utc();

    let first = create_refresh_token(&pool, grant_id, now)
        .await
        .expect("a refresh token");
    let second = create_refresh_token(&pool, grant_id, now)
        .await
        .expect("another one");

    assert_ne!(first, second, "the second replaced the first");

    let stored = sqlx::query!(
        "SELECT grant_id, expires FROM refresh_tokens WHERE token_id = $1",
        Uuid::parse_str(&second).expect("a uuid")
    )
    .fetch_one(&pool)
    .await
    .expect("the row");

    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    assert_eq!(stored.grant_id, Some(grant_id));
    assert_eq!(stored.expires, Some(at + REFRESH_TOKEN_TTL));

    let count = sqlx::query!("SELECT COUNT(*) AS count FROM refresh_tokens")
        .fetch_one(&pool)
        .await
        .expect("count them");

    assert_eq!(count.count, Some(1), "one per grant, not a pile");
}

/// Rotation: the old value is gone the moment a new one is issued, which is the
/// point of handing out a new one at all.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn replacing_a_refresh_token_retires_the_old_one(pool: PgPool) {
    let grant_id = grant(&pool).await;
    let now = OffsetDateTime::now_utc();

    let old = create_refresh_token(&pool, grant_id, now)
        .await
        .expect("a refresh token");

    let replaced = replace_refresh_token(&pool, Uuid::parse_str(&old).unwrap(), now)
        .await
        .expect("an answer")
        .expect("a live token");

    assert_eq!(replaced.0, grant_id);
    assert_ne!(replaced.1, old);

    let again = replace_refresh_token(&pool, Uuid::parse_str(&old).unwrap(), now)
        .await
        .expect("an answer");

    assert_eq!(again, None, "the old one is not a token any more");
}

/// The expiry is part of what is matched, so an expired token cannot be given
/// another six months by presenting it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_expired_refresh_token_cannot_be_replaced(pool: PgPool) {
    let grant_id = grant(&pool).await;
    let now = OffsetDateTime::now_utc();

    let stale = create_refresh_token(&pool, grant_id, now - Duration::days(200))
        .await
        .expect("a refresh token");

    let replaced = replace_refresh_token(&pool, Uuid::parse_str(&stale).unwrap(), now)
        .await
        .expect("an answer");

    assert_eq!(replaced, None);
}

/// The code is spent by taking it, which is what lets the exchange tell a code
/// somebody already redeemed from one that was never issued.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_code_can_only_be_taken_once(pool: PgPool) {
    let application_id = application(&pool).await;
    let client_id = sqlx::query_scalar!(
        "SELECT client_id::text AS \"client_id!\" FROM applications WHERE id = $1",
        application_id
    )
    .fetch_one(&pool)
    .await
    .expect("the client id");

    sqlx::query!(
        "INSERT INTO redirect_uris (application_id, redirect_uri, inserted_at, updated_at)
         VALUES ($1, 'https://app.example/callback', now()::timestamp(0), now()::timestamp(0))",
        application_id
    )
    .execute(&pool)
    .await
    .expect("the redirect uri");

    let now = OffsetDateTime::now_utc();
    let code = authorize(
        &pool,
        42,
        &["openid".to_string()],
        "https://app.example/callback",
        &client_id,
        now,
    )
    .await
    .expect("a code");

    let taken = take_code(&pool, &code)
        .await
        .expect("an answer")
        .expect("the code");

    assert_eq!(taken.application_id, Some(application_id));
    assert_eq!(taken.guild_id, Some(42));
    assert_eq!(
        taken.redirect_uri.as_deref(),
        Some("https://app.example/callback")
    );
    assert_eq!(taken.scopes, ["openid"]);

    let again = take_code(&pool, &code).await.expect("an answer");

    assert_eq!(again, None, "it was spent the first time");
}

/// An application with a registered redirect URI, and its client id.
async fn application_with_callback(pool: &PgPool) -> (i64, String) {
    let application_id = application(pool).await;

    sqlx::query!(
        "INSERT INTO redirect_uris (application_id, redirect_uri, inserted_at, updated_at)
         VALUES ($1, 'https://app.example/callback', now()::timestamp(0), now()::timestamp(0))",
        application_id
    )
    .execute(pool)
    .await
    .expect("the redirect uri");

    let client_id = sqlx::query_scalar!(
        "SELECT client_id::text AS \"client_id!\" FROM applications WHERE id = $1",
        application_id
    )
    .fetch_one(pool)
    .await
    .expect("the client id");

    (application_id, client_id)
}

/// The exchange, and then the same code again — which is the difference the whole
/// refusal taxonomy exists for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_code_is_exchanged_once(pool: PgPool) {
    let (_, client_id) = application_with_callback(&pool).await;
    let now = OffsetDateTime::now_utc();
    let callback = "https://app.example/callback";

    let code = authorize(
        &pool,
        42,
        &["openid".to_string()],
        callback,
        &client_id,
        now,
    )
    .await
    .expect("a code");

    let exchanged = exchange_code(&pool, &client_id, callback, &code, now)
        .await
        .expect("an exchange");

    assert_eq!(exchanged.scopes, ["openid"]);
    assert_eq!(exchanged.expires_in, EXPIRES_IN);
    assert!(
        exchanged.refresh_token.is_none(),
        "the application does not take refresh tokens, so none is issued"
    );

    // The token is a row, and the grant is one grant.
    let tokens = sqlx::query!(
        "SELECT COUNT(*) AS count FROM access_tokens WHERE token_id = $1",
        Uuid::parse_str(&exchanged.access_token).expect("the token is a uuid")
    )
    .fetch_one(&pool)
    .await
    .expect("count the tokens");

    assert_eq!(tokens.count, Some(1));

    let grants = sqlx::query!("SELECT COUNT(*) AS count FROM grants")
        .fetch_one(&pool)
        .await
        .expect("count the grants");

    assert_eq!(grants.count, Some(1));

    // And the code is spent: `used_code`, not `invalid_code`.
    let again = exchange_code(&pool, &client_id, callback, &code, now).await;

    assert_eq!(again, Err(ExchangeError::UsedCode));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_code_that_was_never_issued_is_not_a_reuse(pool: PgPool) {
    let (_, client_id) = application_with_callback(&pool).await;
    let now = OffsetDateTime::now_utc();

    let exchanged = exchange_code(
        &pool,
        &client_id,
        "https://app.example/callback",
        "not-a-code-we-made",
        now,
    )
    .await;

    assert_eq!(exchanged, Err(ExchangeError::InvalidCode));
}

/// Refreshing rotates the refresh token as well, which is what makes a stolen
/// one useless the second time it is presented.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_refresh_retires_the_token_that_was_presented(pool: PgPool) {
    let grant_id = grant(&pool).await;
    let now = OffsetDateTime::now_utc();

    let presented = create_refresh_token(&pool, grant_id, now)
        .await
        .expect("a refresh token");

    let refreshed = exchange_refresh_token(&pool, &presented, now)
        .await
        .expect("a refresh");

    assert_ne!(
        refreshed.refresh_token, presented,
        "the token that was presented is not the one that came back"
    );

    // The access token is a row against the same grant.
    let stored = sqlx::query!(
        "SELECT grant_id FROM access_tokens WHERE token_id = $1",
        Uuid::parse_str(&refreshed.access_token).expect("a uuid")
    )
    .fetch_one(&pool)
    .await
    .expect("the row");

    assert_eq!(stored.grant_id, Some(grant_id));

    // And presenting it again is not a refresh any more.
    let again = exchange_refresh_token(&pool, &presented, now).await;

    assert_eq!(again, Err(ExchangeError::InvalidRefreshToken));
}

/// A refresh token is a row's id, so anything that is not a UUID is not a token
/// rather than a database error.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_refresh_token_that_is_not_a_uuid_is_not_a_token(pool: PgPool) {
    let refreshed = exchange_refresh_token(&pool, "not-a-uuid", OffsetDateTime::now_utc()).await;

    assert_eq!(refreshed, Err(ExchangeError::InvalidRefreshToken));
}

/// Revoking an access token is deleting its row, which is what makes the opaque
/// tokens revocable and a JWT not — the JWT is valid until it expires unless its
/// `jti` row goes.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_an_access_token_empties_its_row(pool: PgPool) {
    let grant_id = grant(&pool).await;
    let now = OffsetDateTime::now_utc();

    let token = create_access_token(&pool, grant_id, now)
        .await
        .expect("a token");

    assert!(revoke_access_token(&pool, &token).await.expect("revoked"));
    // A second time is not a revocation of anything.
    assert!(!revoke_access_token(&pool, &token).await.expect("an answer"));

    let left = sqlx::query!("SELECT COUNT(*) AS count FROM access_tokens")
        .fetch_one(&pool)
        .await
        .expect("count them");

    assert_eq!(left.count, Some(0));
}

/// Revoking a refresh token ends the ability to refresh. The access tokens
/// already issued from that grant are not its business.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_a_refresh_token_stops_it_refreshing(pool: PgPool) {
    let grant_id = grant(&pool).await;
    let now = OffsetDateTime::now_utc();

    let token = create_refresh_token(&pool, grant_id, now)
        .await
        .expect("a refresh token");

    assert!(revoke_refresh_token(&pool, &token).await.expect("revoked"));

    let refreshed = exchange_refresh_token(&pool, &token, now).await;

    assert_eq!(refreshed, Err(ExchangeError::InvalidRefreshToken));
}

/// Anything that is not a token is not a revocation, rather than a database
/// error.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn something_that_is_not_a_token_revokes_nothing(pool: PgPool) {
    assert!(
        !revoke_access_token(&pool, "not-a-uuid")
            .await
            .expect("an answer")
    );
    assert!(
        !revoke_refresh_token(&pool, "not-a-uuid")
            .await
            .expect("an answer")
    );
}

/// An application's webhook is its own: the URL it registered, and the keypair it
/// verifies deliveries with. An application that registered none has one column
/// null, which is an application with nothing to be told.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_applications_webhook_is_read_back(pool: PgPool) {
    let application_id = application(&pool).await;

    sqlx::query!(
        "UPDATE applications SET webhook_url = $2 WHERE id = $1",
        application_id,
        "https://app.example/hook"
    )
    .execute(&pool)
    .await
    .expect("set the webhook url");

    let found = webhook_data(&pool, application_id)
        .await
        .expect("an answer")
        .expect("the application");

    assert_eq!(
        found.webhook_url.as_deref(),
        Some("https://app.example/hook")
    );
    assert_eq!(found.public_key, vec![0u8], "the fixture's key");
    assert_eq!(found.private_key, vec![0u8]);

    let missing = webhook_data(&pool, 999_999).await.expect("an answer");

    assert_eq!(missing, None);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_code_exchange_preserves_code(pool: PgPool) {
    let (_, client) = application_with_callback(&pool).await;
    let now = OffsetDateTime::now_utc();
    let callback = "https://app.example/callback";
    let code = authorize(&pool, 42, &["openid".to_string()], callback, &client, now)
        .await
        .unwrap();
    assert_eq!(
        exchange_code(&pool, &client, "https://wrong.example/", &code, now).await,
        Err(ExchangeError::RedirectUriMismatch)
    );
    let retry = exchange_code(&pool, &client, callback, &code, now).await;
    assert!(
        retry.is_ok(),
        "a rejected request must not consume the code: {retry:?}"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn replayed_code_revokes_issued_tokens(pool: PgPool) {
    let (app, client) = application_with_callback(&pool).await;
    sqlx::query("UPDATE applications SET grant_types=ARRAY['authorization_code','refresh_token']::openid_connect_grant_types[] WHERE id=$1").bind(app).execute(&pool).await.unwrap();
    let now = OffsetDateTime::now_utc();
    let callback = "https://app.example/callback";
    let code = authorize(&pool, 42, &["openid".to_string()], callback, &client, now)
        .await
        .unwrap();
    let tokens = exchange_code(&pool, &client, callback, &code, now)
        .await
        .unwrap();
    assert_eq!(
        exchange_code(&pool, &client, callback, &code, now).await,
        Err(ExchangeError::UsedCode)
    );
    let surviving: i64 = sqlx::query_scalar("SELECT count(*) FROM access_tokens WHERE token_id=$1")
        .bind(Uuid::parse_str(&tokens.access_token).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        surviving, 0,
        "Elixir deletes the grant and cascades revocation on reuse"
    );
    let refresh_survives: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM refresh_tokens WHERE token_id=$1)")
            .bind(Uuid::parse_str(tokens.refresh_token.as_ref().unwrap()).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!refresh_survives, "reuse must revoke the refresh token too");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_refresh_preserves_presented_token(pool: PgPool) {
    let grant_id = grant(&pool).await;
    let now = OffsetDateTime::now_utc();
    let token = create_refresh_token(&pool, grant_id, now).await.unwrap();
    sqlx::query("ALTER TABLE access_tokens ADD CONSTRAINT injected_failure CHECK (false)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(exchange_refresh_token(&pool, &token, now).await.is_err());
    sqlx::query("ALTER TABLE access_tokens DROP CONSTRAINT injected_failure")
        .execute(&pool)
        .await
        .unwrap();
    let retry = exchange_refresh_token(&pool, &token, now).await;
    assert!(
        retry.is_ok(),
        "rotation must roll back when issuing fails: {retry:?}"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn json_revocation_remains_supported(pool: PgPool) {
    use tower::ServiceExt;
    let user = 1234;
    support::insert_user(&pool, user, 12345678).await;
    let token = support::mint(&pool, user, &["vc.pay"]).await;
    let claims = vc_auth::jwt::verify(
        &token,
        support::state(pool.clone(), support::fake()).jwt_secret(),
    )
    .unwrap();
    let response = vc_api::router(support::state(pool.clone(), support::fake()))
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/oauth2/token/revoke")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({"token": token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        200,
        "JSON revocation was supported by Phoenix parsers"
    );
    let survives: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_access_tokens WHERE token_id=$1)")
            .bind(Uuid::parse_str(&claims.jti).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!survives);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_code_token_write_rolls_back_the_grant_and_code(pool: PgPool) {
    let (_, client) = application_with_callback(&pool).await;
    let now = OffsetDateTime::now_utc();
    let callback = "https://app.example/callback";
    let code = authorize(&pool, 42, &["openid".to_string()], callback, &client, now)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE access_tokens ADD CONSTRAINT injected_failure CHECK (false)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        exchange_code(&pool, &client, callback, &code, now)
            .await
            .is_err()
    );
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM grants")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(grants, 0, "a failed exchange must leave no partial grant");
    sqlx::query("ALTER TABLE access_tokens DROP CONSTRAINT injected_failure")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        exchange_code(&pool, &client, callback, &code, now)
            .await
            .is_ok()
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn simultaneous_code_exchanges_detect_reuse_and_revoke(pool: PgPool) {
    let (_, client) = application_with_callback(&pool).await;
    let now = OffsetDateTime::now_utc();
    let callback = "https://app.example/callback";
    let code = authorize(&pool, 42, &["openid".to_string()], callback, &client, now)
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        exchange_code(&pool, &client, callback, &code, now),
        exchange_code(&pool, &client, callback, &code, now)
    );
    assert!(
        matches!(
            (&first, &second),
            (Ok(_), Err(ExchangeError::UsedCode)) | (Err(ExchangeError::UsedCode), Ok(_))
        ),
        "{first:?}, {second:?}"
    );
    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(tokens, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn json_code_exchange_remains_supported(pool: PgPool) {
    use tower::ServiceExt;
    let (_, client) = application_with_callback(&pool).await;
    let callback = "https://app.example/callback";
    let code = authorize(
        &pool,
        42,
        &["openid".to_string()],
        callback,
        &client,
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    let response = vc_api::router(support::state(pool, support::fake()))
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/oauth2/token")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({
                        "grant_type":"authorization_code", "client_id":client,
                        "redirect_uri":callback, "code":code
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["access_token"].is_string());
    assert_eq!(body["token_type"], "Bearer");
}
