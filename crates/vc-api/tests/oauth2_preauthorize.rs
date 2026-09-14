//! `preauthorize`: the four questions asked before a consent screen is shown.
//!
//! There is no Elixir test for any of this — the whole OAuth2 area is the one
//! part of the migration without a ported spec — so these are written from
//! reading `auth/internal/service.ex`, and are additions rather than ports.

use sqlx::PgPool;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;
use vc_core::application::{PreauthorizeError, Preauthorized, authorize, preauthorize};

/// An application with the given grants and registered redirect URIs.
async fn application(
    pool: &PgPool,
    client_id: Uuid,
    grant_types: &[&str],
    redirect_uris: &[&str],
) -> i64 {
    let grants: Vec<String> = grant_types
        .iter()
        .map(|grant| (*grant).to_string())
        .collect();

    let id = sqlx::query_scalar!(
        r#"INSERT INTO applications
             (status, client_id, client_secret, grant_types, client_name,
              public_key, private_key, inserted_at, updated_at)
           VALUES (0, $1, 'secret', $2::text[]::openid_connect_grant_types[],
                   'An Application', '\x00'::bytea, '\x00'::bytea,
                   now()::timestamp(0), now()::timestamp(0))
        RETURNING id"#,
        client_id,
        &grants
    )
    .fetch_one(pool)
    .await
    .expect("insert the application");

    for uri in redirect_uris {
        sqlx::query!(
            "INSERT INTO redirect_uris (application_id, redirect_uri, inserted_at, updated_at)
             VALUES ($1, $2, now()::timestamp(0), now()::timestamp(0))",
            id,
            uri
        )
        .execute(pool)
        .await
        .expect("insert the redirect uri");
    }

    id
}

fn scopes(scopes: &[&str]) -> Vec<String> {
    scopes.iter().map(|scope| (*scope).to_string()).collect()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_valid_request_answers_with_the_name_to_show(pool: PgPool) {
    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    let checked = preauthorize(
        &pool,
        &[],
        "https://app.example/callback",
        &client_id.to_string(),
    )
    .await;

    assert_eq!(
        checked,
        Ok(Preauthorized {
            client_name: Some("An Application".to_owned()),
        })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_client_is_refused(pool: PgPool) {
    let checked = preauthorize(
        &pool,
        &[],
        "https://app.example/callback",
        &Uuid::new_v4().to_string(),
    )
    .await;

    assert_eq!(checked, Err(PreauthorizeError::InvalidClientId));
}

/// The Elixir casts the `client_id` first and only queries a value that survived
/// the cast, so rubbish is a missing client rather than a database error.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_client_id_that_is_not_a_uuid_is_refused(pool: PgPool) {
    let checked = preauthorize(&pool, &[], "https://app.example/callback", "not-a-uuid").await;

    assert_eq!(checked, Err(PreauthorizeError::InvalidClientId));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_redirect_uri_that_was_not_registered_is_refused(pool: PgPool) {
    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    let checked = preauthorize(
        &pool,
        &[],
        "https://app.example/elsewhere",
        &client_id.to_string(),
    )
    .await;

    assert_eq!(checked, Err(PreauthorizeError::InvalidRedirectUri));
}

/// Matching is a string comparison: the Elixir does not normalise, so a trailing
/// slash is a different URI and is not registered.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_redirect_uri_that_merely_resembles_one_is_refused(pool: PgPool) {
    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    let checked = preauthorize(
        &pool,
        &[],
        "https://app.example/callback/",
        &client_id.to_string(),
    )
    .await;

    assert_eq!(checked, Err(PreauthorizeError::InvalidRedirectUri));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_that_cannot_take_a_code_is_refused(pool: PgPool) {
    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["refresh_token"],
        &["https://app.example/callback"],
    )
    .await;

    let checked = preauthorize(
        &pool,
        &[],
        "https://app.example/callback",
        &client_id.to_string(),
    )
    .await;

    assert_eq!(checked, Err(PreauthorizeError::InvalidApplicationGrantType));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_scope_that_is_not_openid_is_refused(pool: PgPool) {
    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    let checked = preauthorize(
        &pool,
        &scopes(&["profile"]),
        "https://app.example/callback",
        &client_id.to_string(),
    )
    .await;

    assert_eq!(checked, Err(PreauthorizeError::InvalidScope));
}

/// The second scope a consent screen may ask for: the one a guild grants an
/// application so that it may issue from the guild's pool.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn vc_issue_is_an_acceptable_scope(pool: PgPool) {
    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    let checked = preauthorize(
        &pool,
        &scopes(&["vc.issue"]),
        "https://app.example/callback",
        &client_id.to_string(),
    )
    .await;

    assert!(checked.is_ok(), "{checked:?}");
}

/// And what an approval records is what a grant carries: `authorize` writes the
/// authorization code whose exchange later writes the grant's scopes, and a grant
/// with `vc.issue` is what a guild token resolves to.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_approved_vc_issue_is_permitted(pool: PgPool) {
    use vc_core::application::authorize;

    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    // Openid is what the consent screen grants by default and `vc.issue` what it
    // grants alongside it: both have to pass `check` for the exchange that follows
    // to write them onto the grant.
    let code = authorize(
        &pool,
        900_000_000_000_000_001,
        &scopes(&["openid", "vc.issue"]),
        "https://app.example/callback",
        &client_id.to_string(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("a code");

    let stored = sqlx::query!(
        r#"SELECT scopes::text[] AS "scopes!" FROM authorization_codes WHERE code = $1"#,
        code
    )
    .fetch_one(&pool)
    .await
    .expect("the code");

    assert!(
        stored.scopes.contains(&"vc.issue".to_owned()),
        "{:?}",
        stored.scopes
    );
}

/// The order is the contract: this request is wrong about two things and is
/// answered by the earlier one.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_first_failure_is_the_one_that_answers(pool: PgPool) {
    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    let checked = preauthorize(
        &pool,
        &scopes(&["profile"]),
        "https://app.example/elsewhere",
        &client_id.to_string(),
    )
    .await;

    assert_eq!(checked, Err(PreauthorizeError::InvalidRedirectUri));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_valid_request_gets_a_code(pool: PgPool) {
    let client_id = Uuid::new_v4();
    let application_id = application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    let now = OffsetDateTime::now_utc();

    let code = authorize(
        &pool,
        42,
        &[],
        "https://app.example/callback",
        &client_id.to_string(),
        now,
    )
    .await
    .expect("a code");

    let stored = sqlx::query!(
        r#"SELECT redirect_uri, application_id, guild_id, scopes::text[] AS "scopes!",
                  expires, inserted_at
             FROM authorization_codes WHERE code = $1"#,
        code
    )
    .fetch_one(&pool)
    .await
    .expect("the row");

    assert_eq!(
        stored.redirect_uri.as_deref(),
        Some("https://app.example/callback")
    );
    assert_eq!(stored.application_id, Some(application_id));
    assert_eq!(stored.guild_id, Some(42));
    assert!(stored.scopes.is_empty(), "{:?}", stored.scopes);

    // Fifteen minutes from the clock it was given, not from some other one — and
    // to the second, which is the precision the column and the Elixir both have.
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();
    assert_eq!(stored.inserted_at, at);
    assert_eq!(stored.expires, Some(at + Duration::minutes(15)));
}

/// A request that would not be preauthorized does not get a code, which is the
/// same four questions asked twice rather than two sets of rules.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_code_that_would_not_be_preauthorized_is_not_made(pool: PgPool) {
    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    let refused = authorize(
        &pool,
        42,
        &[],
        "https://app.example/elsewhere",
        &client_id.to_string(),
        OffsetDateTime::now_utc(),
    )
    .await;

    assert_eq!(refused, Err(PreauthorizeError::InvalidRedirectUri));

    let stored = sqlx::query!("SELECT COUNT(*) AS count FROM authorization_codes")
        .fetch_one(&pool)
        .await
        .expect("count the codes");

    assert_eq!(stored.count, Some(0), "nothing was written");
}

/// Guessing a code is the only way past the consent screen, so two of them must
/// not be the same. Sixty-four hex characters, which is what two UUIDs are.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn two_codes_are_not_the_same_code(pool: PgPool) {
    let client_id = Uuid::new_v4();
    application(
        &pool,
        client_id,
        &["authorization_code"],
        &["https://app.example/callback"],
    )
    .await;

    let one = authorize(
        &pool,
        42,
        &[],
        "https://app.example/callback",
        &client_id.to_string(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("a code");

    let two = authorize(
        &pool,
        42,
        &[],
        "https://app.example/callback",
        &client_id.to_string(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("another code");

    assert_ne!(one, two);
    assert_eq!(one.len(), 64);
}
