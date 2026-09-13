//! `vc_core::user::bind_bot`: the write the connect flow exists to make.
//!
//! It is one `UPDATE`, and the interesting part is what happens when it is not
//! allowed: `users.discord_id` is unique, so a bot that already speaks for another
//! application cannot be claimed by a second one. That branch is the reason this has
//! a test rather than only a caller.

mod support;

use sqlx::PgPool;
use vc_core::user::{BindError, bind_bot};

const BOT: i64 = 500_000_000_000_000_001;

/// An application's account, which is what registration creates: a `users` row with
/// an application and no Discord id of its own.
///
/// The application row has to exist first — `users.application_id` is a foreign key —
/// which is why this is two inserts.
async fn insert_application_account(pool: &PgPool, name: &str) -> i32 {
    let application = sqlx::query_scalar!(
        r#"INSERT INTO applications
             (client_id, client_name, inserted_at, updated_at, public_key, private_key)
           VALUES (gen_random_uuid(), $1, now(), now(), '\x00'::bytea, '\x00'::bytea)
           RETURNING id"#,
        name
    )
    .fetch_one(pool)
    .await
    .expect("an application");

    sqlx::query_scalar!(
        "INSERT INTO users (status, application_id, inserted_at, updated_at)
         VALUES (NULL, $1, now(), now())
         RETURNING id",
        application
    )
    .fetch_one(pool)
    .await
    .expect("the application's account")
}

async fn discord_id_of(pool: &PgPool, user_id: i32) -> Option<i64> {
    sqlx::query_scalar!("SELECT discord_id FROM users WHERE id = $1", user_id)
        .fetch_one(pool)
        .await
        .expect("the account")
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_gives_the_account_the_bots_id(pool: PgPool) {
    let account = insert_application_account(&pool, "one").await;

    assert!(matches!(bind_bot(&pool, account, BOT).await, Ok(())));
    assert_eq!(discord_id_of(&pool, account).await, Some(BOT));
}

/// The branch with a message of its own: 「すでにそのBotは別のApplicationに紐付け
/// られています。」 The database decides it, not a check beforehand, which is why the
/// answer is a named outcome rather than a database error.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bot_another_application_has_is_taken(pool: PgPool) {
    let first = insert_application_account(&pool, "one").await;
    let second = insert_application_account(&pool, "two").await;

    bind_bot(&pool, first, BOT)
        .await
        .expect("the first binding");

    assert!(matches!(
        bind_bot(&pool, second, BOT).await,
        Err(BindError::Taken)
    ));

    // And the first application keeps it: a refused binding is a write that did not
    // happen rather than one that half happened.
    assert_eq!(discord_id_of(&pool, first).await, Some(BOT));
    assert_eq!(discord_id_of(&pool, second).await, None);
}

/// Binding the same application's account again is not a conflict with itself.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_twice_is_not_a_conflict(pool: PgPool) {
    let account = insert_application_account(&pool, "one").await;

    bind_bot(&pool, account, BOT)
        .await
        .expect("the first binding");
    assert!(matches!(bind_bot(&pool, account, BOT).await, Ok(())));
    assert_eq!(discord_id_of(&pool, account).await, Some(BOT));
}
