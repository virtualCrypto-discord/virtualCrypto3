//! Refreshing must use the same grant-before-token lock order as revocation.

mod support;

use sqlx::PgPool;
use std::time::Duration;
use time::OffsetDateTime;
use vc_core::grant::{self, ExchangeError};

async fn wait_for_lock(pool: &PgPool, patterns: &[&str]) {
    let patterns: Vec<String> = patterns.iter().map(|pattern| pattern.to_string()).collect();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                 WHERE datname=current_database() AND wait_event_type='Lock'
                   AND query LIKE ANY($1))",
            )
            .bind(&patterns)
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
    .expect("operation should reach its lock wait");
}

async fn competing(pool: PgPool, revocation_first: bool) {
    let application = support::insert_application(&pool, support::MONEY_USER1, "revocation").await;
    let now = OffsetDateTime::now_utc();
    let grant_id = grant::grant_for_code(&pool, application, 42, "a-code", now)
        .await
        .unwrap()
        .unwrap();
    grant::create_grant_scopes(
        &mut pool.acquire().await.unwrap(),
        grant_id,
        &["vc.issue".into()],
        now,
    )
    .await
    .unwrap();
    let old_access = grant::create_access_token(&pool, grant_id, now)
        .await
        .unwrap();
    let old_refresh = grant::create_refresh_token(&pool, grant_id, now)
        .await
        .unwrap();
    assert!(
        grant::resolve_token(&pool, old_access.parse().unwrap(), now)
            .await
            .unwrap()
            .is_some()
    );

    sqlx::raw_sql(
        "CREATE FUNCTION pause_token_change() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(937281); RETURN NULL; END $$;",
    )
    .execute(&pool)
    .await
    .unwrap();
    let trigger = if revocation_first {
        // Revocation has locked the grant but has not locked its refresh token.
        "CREATE TRIGGER pause_token_change BEFORE DELETE ON refresh_tokens
         FOR EACH STATEMENT EXECUTE FUNCTION pause_token_change()"
    } else {
        // Rotation has locked the refresh token but has not inserted its access token.
        "CREATE TRIGGER pause_token_change AFTER UPDATE ON refresh_tokens
         FOR EACH STATEMENT EXECUTE FUNCTION pause_token_change()"
    };
    sqlx::query(trigger).execute(&pool).await.unwrap();
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(937281)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let refresh = || {
        let pool = pool.clone();
        let token = old_refresh.clone();
        tokio::spawn(async move { grant::exchange_refresh_token(&pool, &token, now).await })
    };
    let revoke = || {
        let pool = pool.clone();
        tokio::spawn(async move { grant::revoke_one(&pool, grant_id, 0, Some(42)).await })
    };
    let (refresh, revocation) = if revocation_first {
        let revocation = revoke();
        wait_for_lock(&pool, &["DELETE FROM grants%"]).await;
        let refresh = refresh();
        wait_for_lock(
            &pool,
            &["SELECT g.id FROM grants%", "INSERT INTO access_tokens%"],
        )
        .await;
        (refresh, revocation)
    } else {
        let refresh = refresh();
        wait_for_lock(&pool, &["UPDATE refresh_tokens%"]).await;
        let revocation = revoke();
        wait_for_lock(&pool, &["DELETE FROM grants%"]).await;
        (refresh, revocation)
    };
    blocker.rollback().await.unwrap();
    let (refreshed, revoked) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(refresh, revocation)
    })
    .await
    .expect("refresh and revocation must finish");
    assert!(revoked.unwrap().expect("revocation must not deadlock"));
    let refreshed = refreshed.unwrap();
    if revocation_first {
        assert_eq!(refreshed, Err(ExchangeError::InvalidRefreshToken));
    } else {
        let refreshed = refreshed.expect("refresh must commit before revocation");
        assert!(
            grant::resolve_token(&pool, refreshed.access_token.parse().unwrap(), now)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            grant::exchange_refresh_token(&pool, &refreshed.refresh_token, now).await,
            Err(ExchangeError::InvalidRefreshToken)
        );
    }
    assert!(
        grant::resolve_token(&pool, old_access.parse().unwrap(), now)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        grant::exchange_refresh_token(&pool, &old_refresh, now).await,
        Err(ExchangeError::InvalidRefreshToken)
    );
    for query in [
        "SELECT count(*) FROM grants WHERE id=$1",
        "SELECT count(*) FROM access_tokens WHERE grant_id=$1",
        "SELECT count(*) FROM refresh_tokens WHERE grant_id=$1",
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .bind(grant_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "revocation must remove the grant and all tokens");
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revocation_after_refresh_invalidates_both_generations_of_tokens(pool: PgPool) {
    competing(pool, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revocation_before_refresh_refuses_the_presented_token(pool: PgPool) {
    competing(pool, true).await;
}
