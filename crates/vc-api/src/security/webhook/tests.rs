use super::*;

use crate::security::{BehaviorMonitor, Kind, Signal};
use axum::{Json, Router, extract::State, response::IntoResponse};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

type Answer = (u16, Option<String>, Value);

struct Wire {
    answers: Mutex<VecDeque<Answer>>,
    bodies: Mutex<Vec<Value>>,
    started: Notify,
    gate: Option<Arc<Semaphore>>,
}

struct Server {
    url: String,
    wire: Arc<Wire>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn server(answers: Vec<Answer>, gate: Option<Arc<Semaphore>>) -> Server {
    async fn answer(
        State(wire): State<Arc<Wire>>,
        Json(body): Json<Value>,
    ) -> axum::response::Response {
        wire.bodies.lock().unwrap().push(body);
        wire.started.notify_one();
        let (status, after, body) = wire
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request");
        if let Some(gate) = &wire.gate {
            gate.acquire().await.unwrap().forget();
        }
        let mut response = (StatusCode::from_u16(status).unwrap(), Json(body)).into_response();
        if let Some(after) = after {
            response
                .headers_mut()
                .insert(reqwest::header::RETRY_AFTER, after.parse().unwrap());
        }
        response
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/api/webhooks/1/path-secret?token=query-secret",
        listener.local_addr().unwrap()
    );
    let wire = Arc::new(Wire {
        answers: Mutex::new(answers.into()),
        bodies: Mutex::new(Vec::new()),
        started: Notify::new(),
        gate,
    });
    let app = Router::new().fallback(answer).with_state(wire.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { url, wire, task }
}

fn report(subject: &str) -> Report {
    Report {
        signal: Signal::AuthFailed,
        subject: subject.into(),
        kind: Kind::First,
        count: 1,
        window_secs: 300,
        detail: "invalid credential".into(),
    }
}

fn sink(pool: &PgPool, url: &str) -> WebhookSink {
    let mut sink = WebhookSink::new(pool.clone(), Some(url.into()));
    sink.destination.as_mut().unwrap().http = Client::builder().no_proxy().build().unwrap();
    sink
}

async fn due(pool: &PgPool) {
    sqlx::query(
        "UPDATE security_webhook_queue SET next_attempt_at = now() WHERE failed_at IS NULL",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn pending(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM security_webhook_queue WHERE failed_at IS NULL")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn queued_reports_survive_a_restart_and_are_removed_after_delivery(pool: PgPool) {
    let server = server(vec![(204, None, Value::Null), (200, None, json!({}))], None).await;
    let old = sink(&pool, &server.url);
    old.enqueue(&[report("first"), report("second")])
        .await
        .unwrap();
    assert_eq!(pending(&pool).await, 2);
    assert!(server.wire.bodies.lock().unwrap().is_empty());
    let stored: Vec<Value> =
        sqlx::query_scalar("SELECT body FROM security_webhook_queue ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        !serde_json::to_string(&stored)
            .unwrap()
            .contains("path-secret")
    );
    drop(old);
    let restarted = sink(&pool, &server.url);
    let destination = restarted.destination.as_ref().unwrap();
    assert!(destination.process_one().await.unwrap());
    assert!(destination.process_one().await.unwrap());
    assert!(!destination.process_one().await.unwrap());
    assert_eq!(pending(&pool).await, 0);
    assert_eq!(*server.wire.bodies.lock().unwrap(), stored);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_429_delay_survives_restart_and_holds_back_later_reports(pool: PgPool) {
    let server = server(
        vec![
            (429, Some("120".into()), json!({})),
            (204, None, Value::Null),
            (204, None, Value::Null),
        ],
        None,
    )
    .await;
    let old = sink(&pool, &server.url);
    old.enqueue(&[report("first"), report("second")])
        .await
        .unwrap();
    assert!(
        old.destination
            .as_ref()
            .unwrap()
            .process_one()
            .await
            .unwrap()
    );
    let row: (i32, i16, f64) = sqlx::query_as("SELECT attempts, last_status, EXTRACT(EPOCH FROM next_attempt_at - now())::float8 FROM security_webhook_queue ORDER BY id LIMIT 1")
        .fetch_one(&pool).await.unwrap();
    assert_eq!((row.0, row.1), (1, 429));
    assert!(row.2 > 115.0 && row.2 <= 120.0, "{row:?}");
    drop(old);
    let restarted = sink(&pool, &server.url);
    let destination = restarted.destination.as_ref().unwrap();
    assert!(!destination.process_one().await.unwrap());
    assert_eq!(server.wire.bodies.lock().unwrap().len(), 1);
    due(&pool).await;
    assert!(destination.process_one().await.unwrap());
    assert!(destination.process_one().await.unwrap());
    {
        let bodies = server.wire.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 3);
        assert_eq!(
            bodies[0], bodies[1],
            "retry preserves the exact saved payload"
        );
        assert_eq!(bodies[2]["subject"], "second");
    }
    assert_eq!(pending(&pool).await, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn discord_json_retry_after_and_server_errors_remain_queued(pool: PgPool) {
    let server = server(
        vec![
            (429, None, json!({"retry_after": 120.5})),
            (503, None, json!({})),
            (204, None, Value::Null),
        ],
        None,
    )
    .await;
    let sink = sink(&pool, &server.url);
    sink.enqueue(&[report("caller")]).await.unwrap();
    let destination = sink.destination.as_ref().unwrap();
    destination.process_one().await.unwrap();
    let wait: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM next_attempt_at - now())::float8 FROM security_webhook_queue",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(wait > 115.5 && wait <= 120.5, "{wait}");
    due(&pool).await;
    destination.process_one().await.unwrap();
    let row: (i32, i16, f64) = sqlx::query_as("SELECT attempts, last_status, EXTRACT(EPOCH FROM next_attempt_at - now())::float8 FROM security_webhook_queue")
        .fetch_one(&pool).await.unwrap();
    assert_eq!((row.0, row.1), (2, 503));
    assert!(row.2 > 1.0 && row.2 <= 2.0, "{row:?}");
    assert_eq!(pending(&pool).await, 1);
    due(&pool).await;
    destination.process_one().await.unwrap();
    assert_eq!(pending(&pool).await, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn permanent_failures_are_retained_and_purged_without_losing_pending_rows(pool: PgPool) {
    let server = server(vec![(400, None, json!({})), (204, None, Value::Null)], None).await;
    let sink = sink(&pool, &server.url);
    sink.enqueue(&[report("bad"), report("good")])
        .await
        .unwrap();
    let destination = sink.destination.as_ref().unwrap();
    destination.process_one().await.unwrap();
    destination.process_one().await.unwrap();
    let failed: (i32, i16) = sqlx::query_as(
        "SELECT attempts, last_status FROM security_webhook_queue WHERE failed_at IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(failed, (1, 400));
    sink.enqueue(&[report("still-pending")]).await.unwrap();
    sqlx::query("UPDATE security_webhook_queue SET inserted_at = now() - interval '30 days', failed_at = CASE WHEN failed_at IS NOT NULL THEN now() - interval '8 days' END")
        .execute(&pool).await.unwrap();
    let now = time::OffsetDateTime::now_utc();
    assert_eq!(
        vc_core::purge::expired(&pool, time::PrimitiveDateTime::new(now.date(), now.time()))
            .await
            .unwrap(),
        1
    );
    assert_eq!(pending(&pool).await, 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn two_workers_cannot_deliver_concurrently(pool: PgPool) {
    let gate = Arc::new(Semaphore::new(0));
    let server = server(
        vec![(204, None, Value::Null), (204, None, Value::Null)],
        Some(gate.clone()),
    )
    .await;
    let first = Arc::new(sink(&pool, &server.url));
    first
        .enqueue(&[report("first"), report("second")])
        .await
        .unwrap();
    let running = first.clone();
    let task = tokio::spawn(async move {
        running
            .destination
            .as_ref()
            .unwrap()
            .process_one()
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(3), server.wire.started.notified())
        .await
        .unwrap();
    let second = sink(&pool, &server.url);
    assert!(
        !second
            .destination
            .as_ref()
            .unwrap()
            .process_one()
            .await
            .unwrap()
    );
    assert_eq!(server.wire.bodies.lock().unwrap().len(), 1);
    gate.add_permits(2);
    assert!(task.await.unwrap());
    assert!(
        second
            .destination
            .as_ref()
            .unwrap()
            .process_one()
            .await
            .unwrap()
    );
    let subjects: Vec<_> = server
        .wire
        .bodies
        .lock()
        .unwrap()
        .iter()
        .map(|body| body["subject"].clone())
        .collect();
    assert_eq!(subjects, [json!("first"), json!("second")]);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancellation_releases_the_claim_and_leaves_the_notification_for_restart(pool: PgPool) {
    let gate = Arc::new(Semaphore::new(0));
    let server = server(
        vec![(204, None, Value::Null), (204, None, Value::Null)],
        Some(gate.clone()),
    )
    .await;
    let old = Arc::new(sink(&pool, &server.url));
    old.enqueue(&[report("retry-after-crash")]).await.unwrap();
    let running = old.clone();
    let task =
        tokio::spawn(async move { running.destination.as_ref().unwrap().process_one().await });
    tokio::time::timeout(Duration::from_secs(3), server.wire.started.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(old);
    assert_eq!(pending(&pool).await, 1);
    gate.add_permits(2);
    let restarted = sink(&pool, &server.url);
    tokio::time::timeout(Duration::from_secs(3), async {
        while !restarted
            .destination
            .as_ref()
            .unwrap()
            .process_one()
            .await
            .unwrap()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(pending(&pool).await, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn background_worker_resumes_existing_rows_without_a_new_notification(pool: PgPool) {
    let server = server(vec![(204, None, Value::Null)], None).await;
    sink(&pool, &server.url)
        .enqueue(&[report("saved")])
        .await
        .unwrap();
    let restarted = sink(&pool, &server.url);
    let worker = tokio::spawn(async move { restarted.run().await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while pending(&pool).await != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(server.wire.bodies.lock().unwrap().len(), 1);
    worker.abort();
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn observation_and_watch_persist_before_returning(pool: PgPool) {
    let monitor = BehaviorMonitor::for_test()
        .with_webhook(pool.clone(), Some("https://example.invalid/hook".into()));
    monitor.observe(Signal::AuthFailed, "caller", "why").await;
    assert_eq!(pending(&pool).await, 1);
    for _ in 0..31 {
        monitor.watch("caller").await;
    }
    assert_eq!(pending(&pool).await, 2);
    assert_eq!(monitor.count_for(Signal::WatchBurst, "caller"), 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn retry_after_is_measured_from_the_response_not_the_transaction_start(pool: PgPool) {
    let gate = Arc::new(Semaphore::new(0));
    let server = server(
        vec![(429, Some("120".into()), json!({}))],
        Some(gate.clone()),
    )
    .await;
    let sink = sink(&pool, &server.url);
    sink.enqueue(&[report("caller")]).await.unwrap();
    let task = tokio::spawn(async move {
        sink.destination
            .as_ref()
            .unwrap()
            .process_one()
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(3), server.wire.started.notified())
        .await
        .unwrap();
    let response_after: time::OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await
        .unwrap();
    gate.add_permits(1);
    assert!(task.await.unwrap());
    let next: time::OffsetDateTime =
        sqlx::query_scalar("SELECT next_attempt_at FROM security_webhook_queue")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(next >= response_after + time::Duration::seconds(120));
}

#[test]
fn retry_after_accepts_seconds_and_http_dates() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    assert_eq!(retry_after("65", now), Some(Duration::from_secs(65)));
    assert_eq!(retry_after("0.25", now), Some(Duration::from_millis(250)));
    assert_eq!(
        retry_after(&httpdate::fmt_http_date(now + Duration::from_secs(90)), now),
        Some(Duration::from_secs(90))
    );
    assert_eq!(
        retry_after(&httpdate::fmt_http_date(now - Duration::from_secs(90)), now),
        Some(Duration::ZERO)
    );
    for invalid in ["", "-1", "NaN", "inf", "1e100", "later"] {
        assert_eq!(retry_after(invalid, now), None);
    }
}

/// Capture the actual sink's log, including its spawned delivery task.
struct Capture(tokio::sync::mpsc::UnboundedSender<String>);

#[derive(Default)]
struct LogFields(String);

impl tracing::field::Visit for LogFields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        write!(&mut self.0, " {field}={value:?}").unwrap();
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "vc_security" {
            let mut fields = LogFields::default();
            event.record(&mut fields);
            let _ = self.0.send(fields.0);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn webhook_transport_errors_do_not_log_url_secrets(pool: PgPool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, mut logs) = tokio::sync::mpsc::unbounded_channel();
    let subscriber = tracing::Dispatch::new(Capture(sender));
    let mut sink = WebhookSink::new(
        pool.clone(),
        Some(format!(
            "http://{address}/api/webhooks/1/path-secret?token=query-secret"
        )),
    );
    sink.destination.as_mut().unwrap().http =
        reqwest::Client::builder().no_proxy().build().unwrap();
    sink.enqueue(&[Report {
        signal: Signal::AuthFailed,
        subject: "unauthenticated".into(),
        kind: Kind::First,
        count: 1,
        window_secs: 300,
        detail: "no credential".into(),
    }])
    .await
    .unwrap();
    use tracing::instrument::WithSubscriber;
    let sending = tokio::spawn(
        async move {
            sink.destination
                .as_ref()
                .unwrap()
                .process_one()
                .await
                .unwrap()
        }
        .with_subscriber(subscriber),
    );

    // Close the connection without a response to force a transport error.
    let (connection, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    drop(connection);
    drop(listener);
    assert!(sending.await.unwrap());
    assert_eq!(pending(&pool).await, 1);
    let log = tokio::time::timeout(Duration::from_secs(10), logs.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        log.contains("the security webhook could not be reached"),
        "{log}"
    );
    assert!(log.contains("auth_failed"), "{log}");
    for secret in ["path-secret", "query-secret", "http://", "/api/webhooks/"] {
        assert!(!log.contains(secret), "{log}");
    }
}
