//! The rows whose time is up are deleted, and the ones that are still good stay.

mod support;

use sqlx::PgPool;
use support::{fake, insert_application, insert_currency, insert_user, state};

const USER: i32 = 1;
const USER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_002;
const GUILD: i64 = 900_000_000_000_000_001;

/// One expired row and one still-good row in each of the three tables, so that
/// what the purge does and what it leaves are both visible.
async fn rows(pool: &PgPool) {
    insert_user(pool, USER, USER_DISCORD_ID).await;
    let application = insert_application(pool, OWNER_DISCORD_ID, "an application").await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 5).await;
    let _ = vc_core::grant::allow_in_guild(
        pool,
        application,
        GUILD,
        &["vc.issue"],
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a grant");
    let grant = vc_core::grant::grant_for(pool, application, GUILD)
        .await
        .expect("a lookup")
        .expect("the grant");

    sqlx::query(
        "INSERT INTO refresh_tokens (grant_id, token_id, expires, inserted_at, updated_at)
         VALUES ($1, gen_random_uuid(), now() - interval '1 second', now(), now())",
    )
    .bind(grant)
    .execute(pool)
    .await
    .expect("a refresh token");

    for (offset, name) in [("-1 second", "gone"), ("1 hour", "live")] {
        sqlx::query(
            "INSERT INTO user_access_tokens (user_id, token_id, expires, inserted_at, updated_at)
             VALUES ($1, gen_random_uuid(), now() + $2::interval, now(), now())",
        )
        .bind(i64::from(USER))
        .bind(offset)
        .execute(pool)
        .await
        .expect("a token row");

        sqlx::query(
            "INSERT INTO payments_idempotency
                 (user_id, idempotency_key, expires, inserted_at, updated_at)
             VALUES ($1, $2::bytea, now() + $3::interval, now(), now())",
        )
        .bind(i64::from(USER))
        .bind(name.as_bytes())
        .bind(offset)
        .execute(pool)
        .await
        .expect("an idempotency row");

        sqlx::query(
            "INSERT INTO access_tokens (grant_id, token_id, expires, inserted_at, updated_at)
             VALUES ($1, gen_random_uuid(), now() + $2::interval, now(), now())",
        )
        .bind(grant)
        .bind(offset)
        .execute(pool)
        .await
        .expect("an access token");

        sqlx::query(
            "INSERT INTO authorization_codes
                 (code, redirect_uri, application_id, guild_id, scopes, expires, inserted_at, updated_at)
             VALUES ($1, 'https://app.example/callback', $2, NULL,
                     ARRAY['vc.issue']::virtual_crypto_scope_type[], now() + $3::interval, now(), now())",
        )
        .bind(name)
        .bind(application)
        .bind(offset)
        .execute(pool)
        .await
        .expect("a code row");
    }
}

/// A count, from a statement the compiler can see: sqlx 0.9 refuses one built at
/// runtime, which is also what keeps a table name out of a format string.
async fn counted(pool: &PgPool, statement: &'static str) -> i64 {
    sqlx::query_scalar(statement)
        .fetch_one(pool)
        .await
        .expect("a count")
}

/// One expired and one still-good row per table, except `refresh_tokens`: it
/// holds one per grant — that is what rotating a refresh token means — so its
/// only row is the expired one.
const COUNTS: [(&str, i64); 5] = [
    ("SELECT count(*) FROM user_access_tokens", 1),
    ("SELECT count(*) FROM payments_idempotency", 1),
    ("SELECT count(*) FROM authorization_codes", 1),
    ("SELECT count(*) FROM access_tokens", 1),
    ("SELECT count(*) FROM refresh_tokens", 0),
];

/// An expired row goes and a live one stays, in every table that carries an
/// `expires` — which is what makes the tables the size of what is still usable.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn expired_rows_go_and_live_ones_stay(pool: PgPool) {
    rows(&pool).await;
    let state = state(pool.clone(), fake());

    vc_api::scheduler::purge_expired(&state).await;

    for (statement, expected) in COUNTS {
        assert_eq!(counted(&pool, statement).await, expected, "{statement}");
    }

    let kept = sqlx::query_scalar!("SELECT code FROM authorization_codes")
        .fetch_one(&pool)
        .await
        .expect("the row that stayed");

    assert_eq!(kept.as_deref(), Some("live"));
}

/// Running it again is a no-op: what is left has time on it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn purging_twice_deletes_nothing_more(pool: PgPool) {
    rows(&pool).await;
    let state = state(pool.clone(), fake());

    vc_api::scheduler::purge_expired(&state).await;
    vc_api::scheduler::purge_expired(&state).await;

    for (statement, expected) in COUNTS {
        assert_eq!(counted(&pool, statement).await, expected, "{statement}");
    }
}
