//! What a deployment would be warned about, read back from the monitor.
//!
//! The log line and the webhook are both products of the same counts, so what
//! a test asserts on is the counts: `count_for` for a subject's signal, and the
//! counters the router records every response into.

mod support;

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use support::{fake, get, insert_discord_auth, insert_user, mint, state_watched};
use vc_api::rate_limit::RateLimiter;
use vc_api::security::{BehaviorMonitor, Signal};

const URI: &str = "/api/v2/users/@me";
const USER_ID: i32 = 1;
const DISCORD_ID: i64 = 100_000_000_000_000_001;

async fn fixture(pool: &PgPool) {
    insert_user(pool, USER_ID, DISCORD_ID).await;
    insert_discord_auth(pool, DISCORD_ID, "stub-token").await;
}

fn unlimited() -> Arc<RateLimiter> {
    Arc::new(RateLimiter::new(0, Duration::from_secs(60)))
}

/// The loose limit refusing a request is the signal the whole design starts
/// from: 429 is unusual enough to say out loud, and said once for the account
/// rather than once per refused request.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_refused_request_is_counted_against_its_account(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER_ID, &["vc.claim"]).await;
    let monitor = Arc::new(BehaviorMonitor::for_test());
    let limiter = Arc::new(RateLimiter::new(1, Duration::from_secs(60)));
    let app = || {
        vc_api::router(state_watched(
            pool.clone(),
            fake(),
            limiter.clone(),
            monitor.clone(),
        ))
    };

    let first = get(app(), URI, Some(&token)).await;
    assert_eq!(first.status, 200, "body: {}", first.body);

    let second = get(app(), URI, Some(&token)).await;
    assert_eq!(second.status, 429, "body: {}", second.body);

    assert_eq!(
        monitor.count_for(Signal::RateLimited, "account:1"),
        1,
        "the refusal is counted for the account it was charged to"
    );
}

/// A credential that resolves to nothing — a malformed bearer, a token this
/// service never issued — is counted where it is refused, under the subject
/// that could not be established.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_credential_that_does_not_resolve_is_counted(pool: PgPool) {
    fixture(&pool).await;
    let monitor = Arc::new(BehaviorMonitor::for_test());
    let app = || {
        vc_api::router(state_watched(
            pool.clone(),
            fake(),
            unlimited(),
            monitor.clone(),
        ))
    };

    let response = get(app(), URI, Some("not-a-jwt")).await;
    assert_eq!(response.status, 401, "body: {}", response.body);

    assert_eq!(monitor.count_for(Signal::AuthFailed, "unauthenticated"), 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_refusal_saves_its_notification_before_returning(pool: PgPool) {
    let monitor = Arc::new(BehaviorMonitor::for_test().with_webhook(
        pool.clone(),
        Some("https://security.example.invalid/webhook/secret".into()),
    ));
    let app = vc_api::router(state_watched(pool.clone(), fake(), unlimited(), monitor));
    let response = get(app, URI, Some("not-a-jwt")).await;
    assert_eq!(response.status, 401);
    let saved: (String, serde_json::Value) =
        sqlx::query_as("SELECT signal, body FROM security_webhook_queue")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(saved.0, "auth_failed");
    assert_eq!(saved.1["subject"], "unauthenticated");
}

/// Every response is a datum for the error rate and the surge, except the
/// heartbeat: this service asking about itself is not callers' behaviour.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn every_response_but_health_is_counted(pool: PgPool) {
    let monitor = Arc::new(BehaviorMonitor::for_test());
    let app = || {
        vc_api::router(state_watched(
            pool.clone(),
            fake(),
            unlimited(),
            monitor.clone(),
        ))
    };

    let response = get(app(), URI, None).await;
    assert!(
        (400..500).contains(&response.status),
        "a refusal is still a response to count: {}",
        response.status
    );
    assert_eq!(monitor.counters().totals(), (1, 0));

    let health = get(app(), "/health", None).await;
    assert_eq!(health.status, 200);
    assert_eq!(
        monitor.counters().totals(),
        (1, 0),
        "the heartbeat is not the callers' behaviour"
    );
}
