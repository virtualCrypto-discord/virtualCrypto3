mod support;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{FakeDiscord, fake, interaction, state};
use tokio::sync::{Notify, Semaphore};
use vc_api::custom_id::ui::developer::{Screen, custom_id_for_field};

const OWNER: i64 = 500_000_000_000_000_001;
const SEED: [u8; 32] = [9; 32];

async fn application(pool: &PgPool) -> (i64, String) {
    support::insert_user(pool, 1, OWNER).await;
    let app = support::insert_application(pool, OWNER, "webhook app").await;
    let public = ed25519_dalek::SigningKey::from_bytes(&SEED)
        .verifying_key()
        .to_bytes();
    sqlx::query("UPDATE applications SET private_key = $2, public_key = $3 WHERE id = $1")
        .bind(app)
        .bind(SEED.to_vec())
        .bind(public.to_vec())
        .execute(pool)
        .await
        .unwrap();
    (app, support::client_id_of(pool, app).await)
}

async fn webhook(pool: &PgPool, app: i64) -> Option<String> {
    sqlx::query_scalar("SELECT webhook_url FROM applications WHERE id = $1")
        .bind(app)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn submission(client_id: &str, url: &str, actor: i64) -> Value {
    json!({
        "type":5, "id":"456", "application_id":"123", "token":"test-token",
        "user":{"id":actor.to_string()},
        "data":{
            "custom_id":custom_id_for_field(Screen::Edit, client_id, "webhook_url"),
            "components":[{"type":18,"component":{
                "type":4,"custom_id":"webhook_url","value":url,
            }}],
        },
    })
}

struct Hook {
    url: String,
    attempts: Arc<AtomicUsize>,
    started: Arc<Notify>,
    gate: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Hook {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn hook(api: Arc<FakeDiscord>, accepts: bool) -> Hook {
    let attempts = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let gate = Arc::new(Semaphore::new(0));
    let (counting, notifying, waiting) = (attempts.clone(), started.clone(), gate.clone());
    let public = ed25519_dalek::SigningKey::from_bytes(&SEED)
        .verifying_key()
        .to_bytes();
    let handler = move |headers: HeaderMap, body: String| {
        let (counting, notifying, waiting, api) = (
            counting.clone(),
            notifying.clone(),
            waiting.clone(),
            api.clone(),
        );
        async move {
            assert_eq!(
                api.callbacks(),
                [json!({"type":5,"data":{"flags":64}})],
                "the interaction must be acknowledged before verification starts"
            );
            counting.fetch_add(1, Ordering::SeqCst);
            notifying.notify_one();
            waiting.acquire().await.unwrap().forget();
            let believed = accepts
                && vc_api::discord::verify_signature(
                    &public,
                    headers["x-signature-ed25519"].to_str().unwrap(),
                    headers["x-signature-timestamp"].to_str().unwrap(),
                    body.as_bytes(),
                );
            (
                if believed {
                    StatusCode::OK
                } else {
                    StatusCode::UNAUTHORIZED
                },
                Json(json!({"type":1})),
            )
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", post(handler)))
            .await
            .unwrap();
    });
    Hook {
        url,
        attempts,
        started,
        gate,
        task,
    }
}

async fn completed(api: &FakeDiscord) -> Value {
    tokio::time::timeout(Duration::from_secs(5), api.response_edit_finished())
        .await
        .expect("the deferred result was delivered");
    let edits = api.response_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["flags"], 32768);
    assert_eq!(edits[0]["allowed_mentions"]["parse"], json!([]));
    edits[0].clone()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn webhook_edit_acknowledges_before_waiting_and_then_updates_the_response(pool: PgPool) {
    let (app, client_id) = application(&pool).await;
    let api = fake();
    let hook = hook(api.clone(), true).await;
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        interaction(
            vc_api::router(state(pool.clone(), api.clone())),
            submission(&client_id, &hook.url, OWNER),
        ),
    )
    .await
    .expect("an initial response must not wait for the webhook");
    assert_eq!(response.status, 202);
    assert_eq!(api.callbacks(), [json!({"type":5,"data":{"flags":64}})]);
    tokio::time::timeout(Duration::from_secs(3), hook.started.notified())
        .await
        .unwrap();
    assert_eq!(
        webhook(&pool, app).await,
        None,
        "no write before verification"
    );
    assert!(api.response_edits().is_empty());

    hook.gate.add_permits(2);
    let result = completed(&api).await;
    assert_eq!(hook.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        webhook(&pool, app).await.as_deref(),
        Some(hook.url.as_str())
    );
    assert!(result.to_string().contains(&hook.url));
    assert!(result.to_string().contains(&client_id));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_verification_updates_the_response_without_changing_the_webhook(pool: PgPool) {
    let (app, client_id) = application(&pool).await;
    sqlx::query("UPDATE applications SET webhook_url = 'https://old.example/hook' WHERE id = $1")
        .bind(app)
        .execute(&pool)
        .await
        .unwrap();
    let api = fake();
    let hook = hook(api.clone(), false).await;
    hook.gate.add_permits(2);
    // Legacy forms without a field in their custom_id still need to defer webhook verification.
    let mut payload = submission(&client_id, &hook.url, OWNER);
    payload["data"]["custom_id"] = json!(vc_api::custom_id::ui::developer::custom_id_for(
        Screen::Edit,
        &client_id
    ));
    let response = interaction(vc_api::router(state(pool.clone(), api.clone())), payload).await;
    assert_eq!(response.status, 202);
    let result = completed(&api).await;
    assert!(
        result.to_string().contains("変更できませんでした"),
        "{result}"
    );
    assert_eq!(
        webhook(&pool, app).await.as_deref(),
        Some("https://old.example/hook")
    );
    assert_eq!(hook.attempts.load(Ordering::SeqCst), 2);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn callback_failure_does_not_start_verification_or_save_settings(pool: PgPool) {
    let (app, client_id) = application(&pool).await;
    let api = FakeDiscord::with_callback_error();
    let hook = hook(api.clone(), true).await;
    let response = interaction(
        vc_api::router(state(pool.clone(), api.clone())),
        submission(&client_id, &hook.url, OWNER),
    )
    .await;
    assert_eq!(response.status, 500);
    assert_eq!(hook.attempts.load(Ordering::SeqCst), 0);
    assert_eq!(webhook(&pool, app).await, None);
    assert!(api.response_edits().is_empty());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deferred_edit_still_requires_application_ownership(pool: PgPool) {
    let (app, client_id) = application(&pool).await;
    let stranger = OWNER + 1;
    support::insert_user(&pool, 3, stranger).await;
    let api = fake();
    let hook = hook(api.clone(), true).await;
    let response = interaction(
        vc_api::router(state(pool.clone(), api.clone())),
        submission(&client_id, &hook.url, stranger),
    )
    .await;
    assert_eq!(response.status, 202);
    let result = completed(&api).await;
    assert!(
        result
            .to_string()
            .contains("そのアプリケーションはありません")
    );
    assert_eq!(hook.attempts.load(Ordering::SeqCst), 0);
    assert_eq!(webhook(&pool, app).await, None);
}
