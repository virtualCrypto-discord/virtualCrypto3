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
use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use support::{fake, insert_application, state};
use time::PrimitiveDateTime;
use tokio::sync::Semaphore;

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
            SET webhook_url = $2, private_key = $3, public_key = $4,
                next_reverify_at = '2000-01-01 00:00:00'::timestamp
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
    let hook = move |headers: axum::http::HeaderMap, body: String| {
        let asked = counting.clone();

        async move {
            asked.fetch_add(1, Ordering::SeqCst);
            honest_reply(&headers, &body)
        }
    };

    (
        serve(axum::Router::new().route("/hook", axum::routing::post(hook))).await,
        asked,
    )
}

fn honest_reply(
    headers: &axum::http::HeaderMap,
    body: &str,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    let public = ed25519_dalek::SigningKey::from_bytes(&SEED).verifying_key();
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
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
        (axum::http::StatusCode::OK, axum::Json(json!({"type": 1})))
    } else {
        (axum::http::StatusCode::UNAUTHORIZED, axum::Json(json!({})))
    }
}

/// Let a settings edit finish while the old endpoint is still answering its check.
async fn paused_hook(passes: bool) -> (String, Arc<Semaphore>, Arc<Semaphore>) {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let seen = entered.clone();
    let gate = release.clone();
    let hook = move |headers: axum::http::HeaderMap, body: String| {
        let seen = seen.clone();
        let gate = gate.clone();
        async move {
            seen.add_permits(1);
            gate.acquire().await.unwrap().forget();
            if passes {
                honest_reply(&headers, &body)
            } else {
                (axum::http::StatusCode::OK, axum::Json(json!({})))
            }
        }
    };
    let url = serve(axum::Router::new().route("/hook", axum::routing::post(hook))).await;
    (url, entered, release)
}

async fn schedule(pool: &PgPool, application: i64) -> Option<PrimitiveDateTime> {
    sqlx::query_scalar("SELECT next_reverify_at FROM applications WHERE id=$1")
        .bind(application)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn snapshot(pool: &PgPool, application: i64) -> vc_core::application::StaleWebhook {
    vc_core::application::stale_webhooks(pool, vc_core::model::utc_now(), 100)
        .await
        .unwrap()
        .into_iter()
        .find(|hook| hook.id == application)
        .unwrap()
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

/// A pass ages: a webhook that answered a week and a day ago is asked again,
/// because "it verified once" is only worth something while it is still true.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_pass_that_is_old_is_asked_again(pool: PgPool) {
    let (url, asked) = honest_hook().await;
    let application = application_at(&pool, &url).await;
    let state = state(pool.clone(), fake());

    vc_api::scheduler::reverify_webhooks(&state).await;

    assert_eq!(asked.load(Ordering::SeqCst), 2, "the two PINGs");

    // The row is due again once its schedule is in the past, which is what the
    // sweep reads: ageing the stored history alone would not move it (`0016`).
    sqlx::query!(
        "UPDATE applications
            SET next_reverify_at = next_reverify_at - interval '8 days'
          WHERE id = $1",
        application
    )
    .execute(&pool)
    .await
    .expect("age the pass");

    vc_api::scheduler::reverify_webhooks(&state).await;

    assert_eq!(asked.load(Ordering::SeqCst), 4, "asked again");
    assert!(
        recorded(&pool, application).await.0.is_some(),
        "and it passed"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_replaced_webhook_keeps_its_schedule_when_the_old_check_fails(pool: PgPool) {
    let (old_url, entered, release) = paused_hook(false).await;
    let (new_url, asked) = honest_hook().await;
    let application = application_at(&pool, &old_url).await;
    let state = state(pool.clone(), fake());
    let sweep_state = state.clone();
    let sweep =
        tokio::spawn(async move { vc_api::scheduler::reverify_webhooks(&sweep_state).await });
    tokio::time::timeout(Duration::from_secs(2), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();

    // Exercise the real update path, including the replacement's successful handshake.
    let changes = vc_core::application::Changes {
        webhook_url: Some(Some(new_url)),
        ..Default::default()
    };
    assert!(
        vc_api::routes::oauth2_clients::apply(&state, application, "replace", &changes)
            .await
            .is_ok()
    );
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    let due = schedule(&pool, application).await;
    release.add_permits(2);
    sweep.await.unwrap();
    assert_eq!(recorded(&pool, application).await, (None, None));
    assert_eq!(schedule(&pool, application).await, due);

    vc_api::scheduler::reverify_webhooks(&state).await;
    assert_eq!(
        asked.load(Ordering::SeqCst),
        4,
        "the replacement is still due immediately"
    );
    assert!(recorded(&pool, application).await.0.is_some());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn removing_a_webhook_discards_an_inflight_success(pool: PgPool) {
    let (url, entered, release) = paused_hook(true).await;
    let application = application_at(&pool, &url).await;
    let state = state(pool.clone(), fake());
    let sweep_state = state.clone();
    let sweep =
        tokio::spawn(async move { vc_api::scheduler::reverify_webhooks(&sweep_state).await });
    tokio::time::timeout(Duration::from_secs(2), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let changes = vc_core::application::Changes {
        webhook_url: Some(None),
        ..Default::default()
    };
    assert!(
        vc_api::routes::oauth2_clients::apply(&state, application, "remove", &changes)
            .await
            .is_ok()
    );
    release.add_permits(2);
    sweep.await.unwrap();
    assert_eq!(recorded(&pool, application).await, (None, None));
    assert_eq!(schedule(&pool, application).await, None);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn restoring_the_same_url_does_not_accept_its_previous_check(pool: PgPool) {
    use vc_core::application::{Changes, patch, record_webhook_verification};
    let url = "https://app.example/hook";
    for intermediate in [Some("https://app.example/replacement"), None] {
        let application = application_at(&pool, url).await;
        let previous = snapshot(&pool, application).await;
        let due = schedule(&pool, application).await;
        for replacement in [intermediate, Some(url)] {
            patch(
                &pool,
                application,
                &Changes {
                    webhook_url: Some(replacement.map(str::to_owned)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        // Emulate edits sharing the schedule's timestamp(0) value. Neither the
        // final URL nor that timestamp can distinguish the obsolete check.
        sqlx::query("UPDATE applications SET next_reverify_at=$2 WHERE id=$1")
            .bind(application)
            .bind(due)
            .execute(&pool)
            .await
            .unwrap();
        for passed in [true, false] {
            assert!(
                !record_webhook_verification(&pool, &previous, passed, vc_core::model::utc_now())
                    .await
                    .unwrap()
            );
        }
        assert_eq!(recorded(&pool, application).await, (None, None));
        assert_eq!(schedule(&pool, application).await, due);
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn overlapping_checks_cannot_overwrite_the_first_completed_result(pool: PgPool) {
    use vc_core::application::record_webhook_verification;
    for passed in [true, false] {
        let application = application_at(&pool, "https://app.example/hook").await;
        let previous = snapshot(&pool, application).await;
        let concurrent = snapshot(&pool, application).await;
        let now = vc_core::model::utc_now();
        assert!(
            record_webhook_verification(&pool, &concurrent, passed, now)
                .await
                .unwrap()
        );
        let kept = recorded(&pool, application).await;
        let due = schedule(&pool, application).await;
        assert!(
            !record_webhook_verification(
                &pool,
                &previous,
                !passed,
                now - time::Duration::seconds(1)
            )
            .await
            .unwrap()
        );
        assert_eq!(recorded(&pool, application).await, kept);
        assert_eq!(schedule(&pool, application).await, due);
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn metadata_edits_and_an_unchanged_url_preserve_the_pending_check(pool: PgPool) {
    use vc_core::application::{Changes, patch, record_webhook_verification};
    let url = "https://app.example/hook";
    for unchanged_url in [None, Some(Some(url.to_owned()))] {
        let application = application_at(&pool, url).await;
        let previous = snapshot(&pool, application).await;
        let due = schedule(&pool, application).await;
        patch(
            &pool,
            application,
            &Changes {
                client_name: Some(Some("renamed application".to_owned())),
                webhook_url: unchanged_url,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(schedule(&pool, application).await, due);
        assert!(
            record_webhook_verification(&pool, &previous, true, vc_core::model::utc_now())
                .await
                .unwrap()
        );
        assert!(recorded(&pool, application).await.0.is_some());
    }
}
