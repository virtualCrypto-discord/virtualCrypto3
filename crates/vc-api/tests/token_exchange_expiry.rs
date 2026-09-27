//! Advancing the clock while a row is locked must not extend a credential's life.

mod support;

use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use time::{OffsetDateTime, PrimitiveDateTime};
use vc_core::grant::{self, ExchangeError};

async fn wait_for_lock(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                 WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)",
            )
            .bind(pattern)
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the exchange must reach the held lock");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn code_expiring_during_its_lock_wait_is_not_consumed(pool: PgPool) {
    let application = support::insert_application(&pool, support::MONEY_USER1, "expiry").await;
    sqlx::query("UPDATE applications SET grant_types=ARRAY['authorization_code','refresh_token']::openid_connect_grant_types[] WHERE id=$1")
        .bind(application).execute(&pool).await.unwrap();
    let client: String = sqlx::query_scalar("SELECT client_id::text FROM applications WHERE id=$1")
        .bind(application)
        .fetch_one(&pool)
        .await
        .unwrap();
    let callback = "https://app.example/callback";
    sqlx::query("INSERT INTO redirect_uris(application_id,redirect_uri,inserted_at,updated_at) VALUES($1,$2,now(),now())")
        .bind(application).bind(callback).execute(&pool).await.unwrap();
    let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
    let code = vc_core::application::authorize(
        &pool,
        42,
        &["vc.issue".into()],
        &[],
        callback,
        &client,
        now,
    )
    .await
    .unwrap();
    let expires: PrimitiveDateTime =
        sqlx::query_scalar("SELECT expires FROM authorization_codes WHERE code=$1")
            .bind(&code)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM authorization_codes WHERE code=$1 FOR UPDATE")
        .bind(&code)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let clock = Arc::new(Mutex::new(now));
    let exchange = {
        let pool = pool.clone();
        let code = code.clone();
        let clock = clock.clone();
        tokio::spawn(async move {
            grant::exchange_code(&pool, &client, callback, &code, || *clock.lock().unwrap()).await
        })
    };
    wait_for_lock(&pool, "DELETE FROM authorization_codes%").await;
    *clock.lock().unwrap() = expires.assume_utc() + time::Duration::seconds(1);
    blocker.rollback().await.unwrap();
    assert_eq!(exchange.await.unwrap(), Err(ExchangeError::InvalidCode));
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM authorization_codes WHERE code=$1")
            .bind(code)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 1, "refusal must roll back consumption");
    for query in [
        "SELECT count(*) FROM grants",
        "SELECT count(*) FROM access_tokens",
        "SELECT count(*) FROM refresh_tokens",
    ] {
        let count: i64 = sqlx::query_scalar(query).fetch_one(&pool).await.unwrap();
        assert_eq!(
            count, 0,
            "refusal must not create grants or tokens: {query}"
        );
    }
}

async fn refresh_expiring_behind_lock(pool: PgPool, lock_grant: bool) {
    let application = support::insert_application(&pool, support::MONEY_USER1, "expiry").await;
    let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
    let grant = grant::grant_for_code(&pool, application, 42, "code", now)
        .await
        .unwrap()
        .unwrap();
    let token = grant::create_refresh_token(&pool, grant, now)
        .await
        .unwrap();
    let expires: PrimitiveDateTime =
        sqlx::query_scalar("SELECT expires FROM refresh_tokens WHERE grant_id=$1")
            .bind(grant)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut blocker = pool.begin().await.unwrap();
    let (lock, waiting) = if lock_grant {
        (
            "SELECT id FROM grants WHERE id=$1 FOR UPDATE",
            "SELECT g.id FROM grants%",
        )
    } else {
        (
            "SELECT id FROM refresh_tokens WHERE grant_id=$1 FOR UPDATE",
            "%FROM refresh_tokens%",
        )
    };
    sqlx::query(lock)
        .bind(grant)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let clock = Arc::new(Mutex::new(now));
    let exchange = {
        let pool = pool.clone();
        let token = token.clone();
        let clock = clock.clone();
        tokio::spawn(async move {
            grant::exchange_refresh_token(&pool, &token, || *clock.lock().unwrap()).await
        })
    };
    wait_for_lock(&pool, waiting).await;
    *clock.lock().unwrap() = expires.assume_utc() + time::Duration::seconds(1);
    blocker.rollback().await.unwrap();
    assert_eq!(
        exchange.await.unwrap(),
        Err(ExchangeError::InvalidRefreshToken)
    );
    let stored: String =
        sqlx::query_scalar("SELECT token_id::text FROM refresh_tokens WHERE grant_id=$1")
            .bind(grant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, token, "refusal must not rotate the token");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "an expired refresh token must not mint access tokens"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn refresh_expiring_while_waiting_for_its_grant_is_refused(pool: PgPool) {
    refresh_expiring_behind_lock(pool, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn refresh_expiring_while_waiting_for_its_token_is_refused(pool: PgPool) {
    refresh_expiring_behind_lock(pool, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn concurrent_refreshes_rotate_once(pool: PgPool) {
    let application = support::insert_application(&pool, support::MONEY_USER1, "expiry").await;
    let now = OffsetDateTime::now_utc();
    let grant = grant::grant_for_code(&pool, application, 42, "code", now)
        .await
        .unwrap()
        .unwrap();
    let token = grant::create_refresh_token(&pool, grant, now)
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        grant::exchange_refresh_token(&pool, &token, OffsetDateTime::now_utc),
        grant::exchange_refresh_token(&pool, &token, OffsetDateTime::now_utc),
    );
    let (winner, loser) = if first.is_ok() {
        (first, second)
    } else {
        (second, first)
    };
    let winner = winner.unwrap();
    assert_eq!(loser, Err(ExchangeError::InvalidRefreshToken));
    let stored: String =
        sqlx::query_scalar("SELECT token_id::text FROM refresh_tokens WHERE grant_id=$1")
            .bind(grant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, winner.refresh_token);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM access_tokens WHERE grant_id=$1")
        .bind(grant)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
