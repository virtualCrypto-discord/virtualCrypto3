//! The tokens a grant owns.
//!
//! Additions rather than ports: OAuth2 is the part of the migration with no
//! Elixir test to follow.

use sqlx::PgPool;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;
use vc_core::application::{authorize, take_code};
use vc_core::grant::{
    EXPIRES_IN, ExchangeError, REFRESH_TOKEN_TTL, create_access_token, create_grant_scopes,
    create_refresh_token, exchange_code, exchange_refresh_token, grant_for_code,
    replace_refresh_token,
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

    create_grant_scopes(&pool, grant_id, &["openid".to_string()], now)
        .await
        .expect("the scopes");
    create_grant_scopes(&pool, grant_id, &["openid".to_string()], now)
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
