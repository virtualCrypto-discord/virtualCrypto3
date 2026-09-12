//! The tokens a grant owns.
//!
//! Additions rather than ports: OAuth2 is the part of the migration with no
//! Elixir test to follow.

use sqlx::PgPool;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;
use vc_core::grant::create_access_token;

/// An application, and a grant of it in a guild.
async fn grant(pool: &PgPool) -> i64 {
    let application_id = sqlx::query_scalar!(
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
    .expect("insert the application");

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
