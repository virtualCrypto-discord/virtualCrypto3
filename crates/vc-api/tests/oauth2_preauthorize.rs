//! `preauthorize`: the four questions asked before a consent screen is shown.
//!
//! There is no Elixir test for any of this — the whole OAuth2 area is the one
//! part of the migration without a ported spec — so these are written from
//! reading `auth/internal/service.ex`, and are additions rather than ports.

use sqlx::PgPool;
use uuid::Uuid;
use vc_core::application::{PreauthorizeError, Preauthorized, preauthorize};

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
