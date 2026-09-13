//! `GET /oauth2/clients/@me`: which applications the caller owns.
//!
//! Ownership is by Discord id — `applications.owner_discord_id` against the
//! account's `discord_id` — and not by `users.application_id`, which points the
//! other way. The two are easy to confuse and were confused here: this endpoint used
//! to answer an empty list to a person who owned an application, which is why the
//! test below is about a person who owns one.
//!
//! These assert the body rather than comparing it to a golden. No golden for this
//! endpoint was ever captured from the Elixir, and writing one now would be writing
//! down this implementation's answer and calling it the other one's.

mod support;

use sqlx::PgPool;
use support::{fake, get, insert_application, insert_user, mint, mint_app, state};

const URI: &str = "/oauth2/clients";

// The person, and the account created for their application. The second has no
// discord id, which is what makes it the application's account rather than
// anybody's.
const OWNER_ID: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_owner_is_answered_their_application(pool: PgPool) {
    insert_user(&pool, OWNER_ID, OWNER_DISCORD_ID).await;
    insert_application(&pool, OWNER_DISCORD_ID, "one").await;

    let token = mint(&pool, OWNER_ID, &["oauth2.register"]).await;
    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 200, "{:?}", response.body);

    let applications = response.body.as_array().expect("an array");
    assert_eq!(applications.len(), 1);
    assert_eq!(applications[0]["client_name"], "one");
    assert_eq!(
        applications[0]["owner_discord_id"],
        OWNER_DISCORD_ID.to_string()
    );
}

/// Several, because nothing makes an owner's applications unique. This is the case
/// the old lookup could not have answered at all: it read a single-valued column.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_owner_may_be_answered_several(pool: PgPool) {
    insert_user(&pool, OWNER_ID, OWNER_DISCORD_ID).await;
    insert_application(&pool, OWNER_DISCORD_ID, "one").await;
    insert_application(&pool, OWNER_DISCORD_ID, "two").await;

    let token = mint(&pool, OWNER_ID, &["oauth2.register"]).await;
    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 200, "{:?}", response.body);

    let applications = response.body.as_array().expect("an array");
    assert_eq!(applications.len(), 2);
    assert_eq!(applications[0]["client_name"], "one");
    assert_eq!(applications[1]["client_name"], "two");
}

/// Somebody else's application is not this account's.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn another_owners_application_is_not_answered(pool: PgPool) {
    insert_user(&pool, OWNER_ID, OWNER_DISCORD_ID).await;
    insert_application(&pool, 500_000_000_000_000_009, "theirs").await;

    let token = mint(&pool, OWNER_ID, &["oauth2.register"]).await;
    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 200, "{:?}", response.body);
    assert_eq!(response.body, serde_json::json!([]));
}

/// An application's own token may not ask this. The Elixir refuses it 401
/// `invalid_token` / `invalid_kind`, and both halves of that spelling are asserted,
/// because `invalid_token` alone would also be a token that is simply bad.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_token_is_refused(pool: PgPool) {
    insert_user(&pool, OWNER_ID, OWNER_DISCORD_ID).await;
    let application_id = insert_application(&pool, OWNER_DISCORD_ID, "one").await;

    // The account registration created for the application, which is the subject an
    // app token carries.
    let application_user: i32 = sqlx::query_scalar!(
        "SELECT id FROM users WHERE application_id = $1",
        application_id
    )
    .fetch_one(&pool)
    .await
    .expect("the application's account");

    let token = mint_app(&pool, application_user, &["oauth2.register"]).await;
    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 401, "{:?}", response.body);
    assert_eq!(response.body["error"], "invalid_token");
    assert_eq!(response.body["error_description"], "invalid_kind");
}
