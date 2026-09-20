//! The clock's fourth job: the webhooks that have stopped answering.
//!
//! The handshake runs at registration and at an edit, and a webhook that passed
//! once and then went quiet is one nobody hears about — a delivery is
//! fire-and-forget, so nothing about sending one says whether it landed. What
//! these pin is the whole job: a pass is recorded, a failure is recorded beside
//! it, a webhook that has just been checked is left alone, and a fleet is checked
//! a batch at a time rather than all at once.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use sqlx::PgPool;
use support::{fake, insert_application, state};
use time::PrimitiveDateTime;

const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;

/// A seed, because a handshake is signed with the application's own key and one
/// that is not thirty-two bytes is skipped rather than signed with a guess.
const SEED: [u8; 32] = [9u8; 32];

/// How many webhooks one pass re-checks, which is the job's own batch size.
const PER_PASS: usize = 10;

/// An application whose webhook is `url`.
async fn application_at(pool: &PgPool, url: &str) -> i64 {
    let application = insert_application(pool, OWNER_DISCORD_ID, "an application").await;
    let signing = ed25519_dalek::SigningKey::from_bytes(&SEED);

    sqlx::query!(
        "UPDATE applications
            SET webhook_url = $2, private_key = $3, public_key = $4
          WHERE id = $1",
        application,
        url,
        signing.to_bytes().to_vec(),
        signing.verifying_key().to_bytes().to_vec()
    )
    .execute(pool)
    .await
    .expect("the webhook");

    application
}

/// What the clock last found, as the columns hold it.
async fn recorded(
    pool: &PgPool,
    application: i64,
) -> (Option<PrimitiveDateTime>, Option<PrimitiveDateTime>) {
    let row = sqlx::query!(
        "SELECT webhook_verified_at, webhook_failed_at FROM applications WHERE id = $1",
        application
    )
    .fetch_one(pool)
    .await
    .expect("the application");

    (row.webhook_verified_at, row.webhook_failed_at)
}

/// An application, as far as a handshake can tell: it verifies the signature the
/// way its own documentation says, answers the PING it believes, and refuses the
/// one it does not. It counts what it is asked, which is how a test tells a
/// re-check from a webhook that was left alone.
async fn honest_hook() -> (String, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let counting = asked.clone();
    let signing = ed25519_dalek::SigningKey::from_bytes(&SEED);
    let public = signing.verifying_key();

    let hook = move |headers: axum::http::HeaderMap, body: String| {
        let asked = counting.clone();

        async move {
            asked.fetch_add(1, Ordering::SeqCst);

            let header = |name: &str| {
                headers
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned()
            };

            let signature = header("X-Signature-Ed25519");
            let timestamp = header("X-Signature-Timestamp");

            let bytes: Option<[u8; 64]> = (0..signature.len() / 2)
                .map(|pair| u8::from_str_radix(&signature[pair * 2..pair * 2 + 2], 16).ok())
                .collect::<Option<Vec<_>>>()
                .and_then(|bytes| bytes.try_into().ok());

            let believed = bytes
                .map(|bytes| ed25519_dalek::Signature::from_bytes(&bytes))
                .is_some_and(|signature| {
                    ed25519_dalek::Verifier::verify(
                        &public,
                        format!("{timestamp}{body}").as_bytes(),
                        &signature,
                    )
                    .is_ok()
                });

            if believed {
                (axum::http::StatusCode::OK, axum::Json(json!({ "type": 1 })))
            } else {
                (axum::http::StatusCode::UNAUTHORIZED, axum::Json(json!({})))
            }
        }
    };

    (
        serve(axum::Router::new().route("/hook", axum::routing::post(hook))).await,
        asked,
    )
}

/// A webhook that answers everything with `200` and an empty body: it is there,
/// and it verifies nothing. That is a `Failed` handshake — the same answer an
/// application that never checks a signature gives.
async fn indifferent_hook() -> String {
    let hook =
        move |_body: String| async move { (axum::http::StatusCode::OK, axum::Json(json!({}))) };

    serve(axum::Router::new().route("/hook", axum::routing::post(hook))).await
}

/// An address nothing is listening on: a port that was bound and then let go, so
/// a connection to it is refused rather than routed somewhere.
async fn nothing_listening() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("the address");

    drop(listener);

    format!("http://{address}/hook")
}

async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("the address");

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    format!("http://{address}/hook")
}

/// A webhook that answers is a webhook that has been checked, and one that was
/// just checked is not asked again on the next tick.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_webhook_that_answers_is_recorded_and_left_alone(pool: PgPool) {
    let (url, asked) = honest_hook().await;
    let application = application_at(&pool, &url).await;
    let state = state(pool.clone(), fake());

    vc_api::scheduler::reverify_webhooks(&state).await;

    let (verified, failed) = recorded(&pool, application).await;

    assert!(verified.is_some(), "the pass was recorded");
    assert_eq!(failed, None, "and nothing failed");
    assert_eq!(asked.load(Ordering::SeqCst), 2, "the two PINGs");

    vc_api::scheduler::reverify_webhooks(&state).await;

    assert_eq!(
        asked.load(Ordering::SeqCst),
        2,
        "a webhook checked a moment ago is not checked again"
    );
    assert_eq!(recorded(&pool, application).await.0, verified);
}

/// A webhook that answers but verifies nothing fails the handshake, and the
/// failure is written beside the pass rather than over it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_webhook_that_verifies_nothing_is_recorded_as_failed(pool: PgPool) {
    let url = indifferent_hook().await;
    let application = application_at(&pool, &url).await;
    let state = state(pool.clone(), fake());

    vc_api::scheduler::reverify_webhooks(&state).await;

    let (verified, failed) = recorded(&pool, application).await;

    assert_eq!(verified, None, "it never passed");
    assert!(failed.is_some(), "and the failure is the record");
}

/// A webhook nobody answers is a failure too, and it keeps its URL: a delivery is
/// a decision about somebody's money, and a silence is not a reason to stop
/// telling an application about it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_silent_webhook_fails_and_keeps_its_url(pool: PgPool) {
    let url = nothing_listening().await;
    let application = application_at(&pool, &url).await;
    let state = state(pool.clone(), fake());

    vc_api::scheduler::reverify_webhooks(&state).await;

    let (verified, failed) = recorded(&pool, application).await;

    assert_eq!(verified, None);
    assert!(failed.is_some(), "a silence is a failure");

    let kept = sqlx::query_scalar!(
        "SELECT webhook_url FROM applications WHERE id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the application");

    assert_eq!(kept.as_deref(), Some(url.as_str()), "the webhook stays");
}

/// The pass is a batch: a fleet bigger than one pass is checked over several of
/// them, never-checked first, rather than all in one tick.
///
/// This is also what says the job does not charge the handshake limiter: that
/// budget allows one handshake per three seconds per requester, so a pass that
/// spent it would check one webhook and be refused the other nine — and the count
/// below would be one rather than ten.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_fleet_is_checked_a_batch_at_a_time(pool: PgPool) {
    let url = nothing_listening().await;
    let state = state(pool.clone(), fake());

    let mut applications = Vec::new();
    for _ in 0..PER_PASS + 2 {
        applications.push(application_at(&pool, &url).await);
    }

    vc_api::scheduler::reverify_webhooks(&state).await;

    let mut checked = 0;
    for application in &applications {
        if recorded(&pool, *application).await.1.is_some() {
            checked += 1;
        }
    }

    assert_eq!(checked, PER_PASS, "one pass, one batch");

    vc_api::scheduler::reverify_webhooks(&state).await;

    for application in &applications {
        assert!(
            recorded(&pool, *application).await.1.is_some(),
            "the next pass takes the rest"
        );
    }
}

/// An application that named no webhook has nothing to check, and is not selected
/// at all.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_without_a_webhook_is_not_checked(pool: PgPool) {
    let application = insert_application(&pool, OWNER_DISCORD_ID, "an application").await;
    let state = state(pool.clone(), fake());

    vc_api::scheduler::reverify_webhooks(&state).await;

    let (verified, failed) = recorded(&pool, application).await;

    assert_eq!(verified, None);
    assert_eq!(failed, None, "nothing to check is not a failure");
}
