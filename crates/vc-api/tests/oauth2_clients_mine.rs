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
use support::{fake, get, insert_user, mint, state};

const URI: &str = "/oauth2/clients/@me";

// The person, and the account created for their application. The second has no
// discord id, which is what makes it the application's account rather than
// anybody's.
const OWNER_ID: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;

/// An application owned by `owner_discord_id`, and the account created for it.
///
/// Both rows are needed: the read joins through the account, so an application
/// whose account is missing is an application this endpoint cannot see.
async fn insert_application(pool: &PgPool, owner_discord_id: i64, name: &str) -> i64 {
    let id = sqlx::query_scalar!(
        r#"INSERT INTO applications
             (client_id, client_name, owner_discord_id, inserted_at, updated_at,
              public_key, private_key)
           VALUES (gen_random_uuid(), $1, $2, now(), now(), '\x00'::bytea, '\x00'::bytea)
           RETURNING id"#,
        name,
        owner_discord_id
    )
    .fetch_one(pool)
    .await
    .expect("an application");

    sqlx::query!(
        "INSERT INTO users (status, application_id, inserted_at, updated_at)
         VALUES (NULL, $1, now(), now())",
        id
    )
    .execute(pool)
    .await
    .expect("the application's account");

    id
}

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
