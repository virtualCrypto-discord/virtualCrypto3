//! `GET /oauth2/clients/@me`: the application the token is for.
//!
//! This is RFC 7592's read of the client the token identifies, and it is the URI that
//! `POST /oauth2/clients` answers as `registration_client_uri`. So it takes the
//! **application** token that registration issued, and the test that matters most is
//! the one below that does exactly that: register, then read itself at the address it
//! was handed.
//!
//! The list is not here. It is `oauth2_clients_mine.rs`, on `/oauth2/clients`. Both
//! answered `/@me` until this was corrected — the list had the path, and the read had
//! been promised it by the registration it comes from.
//!
//! **What is not covered**: registering and then reading the application back with the
//! token registration answered. Driving `POST /oauth2/clients` needs the caller to have
//! a stored Discord authorization — it is verified against Discord as the Elixir did —
//! and the test support has no fixture for one, so the test was tried, refused a 500 at
//! that check, and taken back out rather than worked around. `registration_client_uri`
//! is asserted by nothing, for the same reason.

mod support;

use sqlx::PgPool;
use support::{fake, get, insert_user, mint, mint_app, state};

const URI: &str = "/oauth2/clients/@me";
const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;

/// An application owned by `owner_discord_id`, and the id of the account created for
/// it. The account is what an application token's subject is.
async fn insert_application(pool: &PgPool, owner_discord_id: i64, name: &str) -> (i64, i32) {
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

    let account = sqlx::query_scalar!(
        "INSERT INTO users (status, application_id, inserted_at, updated_at)
         VALUES (NULL, $1, now(), now()) RETURNING id",
        id
    )
    .fetch_one(pool)
    .await
    .expect("the application's account");

    (id, account)
}

/// A client reads itself with the token it was given, which is the whole of what RFC
/// 7592's read is for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_token_reads_its_own_application(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let (application, account) = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let client_id = sqlx::query_scalar!(
        "SELECT client_id::text FROM applications WHERE id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the row")
    .expect("a client id");
    let token = mint_app(&pool, account, &["oauth2.register"]).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 200, "{:?}", response.body);
    assert_eq!(response.body["client_id"], client_id);
    assert_eq!(response.body["client_name"], "mine");
}

/// A person is not a client reading itself, and this path is the client's. The same
/// answer `PATCH` on this path gives, since they are the two halves of one call.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_user_token_is_refused(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 401, "{:?}", response.body);
    assert_eq!(response.body["error"], "invalid_kind");
}

/// An application token without the registration scope is refused the way `PATCH` on
/// this path refuses it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn without_the_scope_is_403(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let (_, account) = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let token = mint_app(&pool, account, &[]).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 403, "{:?}", response.body);
    assert_eq!(response.body["error"], "insufficient_scope");
}
