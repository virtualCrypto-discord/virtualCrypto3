//! Exercise the executable, including retries, charge keys and statement totals.

use axum::{Json, Router, extract::Request, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct Reply {
    method: &'static str,
    uri: String,
    key: Option<String>,
    status: u16,
    body: Value,
}

fn reply(method: &'static str, uri: &str, status: u16, body: Value) -> Reply {
    Reply {
        method,
        uri: uri.into(),
        key: None,
        status,
        body,
    }
}

fn charged(use_number: i64, quota: i64) -> Reply {
    Reply {
        key: Some(format!("\"1-{use_number}\"")),
        ..reply(
            "POST",
            "/api/v2/contracts/1/payments",
            201,
            json!({"amount":"1", "unit":"n", "remaining":(quota-use_number).to_string()}),
        )
    }
}

fn append(script: &mut Vec<Reply>, response: Reply, rate_limited: bool) {
    if rate_limited {
        script.push(Reply {
            method: response.method,
            uri: response.uri.clone(),
            key: response.key.clone(),
            status: 429,
            body: json!({"error":"rate_limited"}),
        });
    }
    script.push(response);
}

fn start(rate_limited: bool) -> Vec<Reply> {
    let mut script = vec![reply(
        "POST",
        "/oauth2/token",
        200,
        json!({"access_token":"app-token", "token_type":"Bearer"}),
    )];
    append(
        &mut script,
        reply(
            "POST",
            "/api/v2/contracts",
            201,
            json!({"id":"1", "status":"pending"}),
        ),
        rate_limited,
    );
    append(
        &mut script,
        reply(
            "GET",
            "/api/v2/contracts/1",
            200,
            json!({"id":"1", "status":"active"}),
        ),
        rate_limited,
    );
    script
}

/// A full statement also contains a lock and a refund. Neither is a charge.
fn ledger(charges: i64, quota: i64) -> Vec<Value> {
    let mut rows = vec![json!({"id":(charges+2).to_string(), "event":"return", "amount":"7"})];
    rows.extend(
        (2..=charges + 1)
            .rev()
            .map(|id| json!({"id":id.to_string(), "event":"charge", "amount":"1"})),
    );
    rows.push(json!({"id":"1", "event":"lock", "amount":quota.to_string()}));
    rows
}

async fn run(replies: Vec<Reply>, uses: i64, quota: i64) -> String {
    struct Script {
        replies: VecDeque<Reply>,
        retry_at: Option<Instant>,
    }
    let script = Arc::new(Mutex::new(Script {
        replies: replies.into(),
        retry_at: None,
    }));
    let handler = script.clone();
    let app = Router::new().fallback(move |request: Request| {
        let script = handler.clone();
        async move {
            let method = request.method().to_string();
            let uri = request.uri().to_string();
            let key = request
                .headers()
                .get("idempotency-key")
                .map(|key| key.to_str().unwrap().to_owned());
            if uri.starts_with("/api/") {
                assert_eq!(request.headers()["authorization"], "Bearer app-token");
            }
            let body = axum::body::to_bytes(request.into_body(), 4096)
                .await
                .unwrap();
            let mut script = script.lock().unwrap();
            if let Some(retry_at) = script.retry_at.take() {
                assert!(Instant::now() >= retry_at, "client ignored Retry-After");
            }
            let expected = script.replies.pop_front().expect("unexpected request");
            assert_eq!(
                (method.as_str(), uri.as_str()),
                (expected.method, expected.uri.as_str())
            );
            assert_eq!(key, expected.key);
            if method == "POST" && uri.ends_with("/payments") {
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap(),
                    json!({"receiver_discord_id":"2", "amount":"1"})
                );
            }
            let mut response = (
                StatusCode::from_u16(expected.status).unwrap(),
                Json(expected.body),
            )
                .into_response();
            if expected.status == 429 {
                response
                    .headers_mut()
                    .insert("retry-after", "1".parse().unwrap());
                script.retry_at = Some(Instant::now() + Duration::from_secs(1));
            }
            response
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_demo-billing"))
            .args([
                "--service",
                &service,
                "--client-id",
                "test-client",
                "--client-secret",
                "test-secret",
                "--discord-id",
                "1",
                "--unit",
                "n",
                "--quota",
                &quota.to_string(),
                "--price",
                "1",
                "--uses",
                &uses.to_string(),
                "--receiver-id",
                "2",
            ])
            .env("NO_PROXY", "127.0.0.1")
            .output()
            .unwrap()
    });
    let output = tokio::time::timeout(Duration::from_secs(20), client)
        .await
        .unwrap()
        .unwrap();
    server.abort();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        script.lock().unwrap().replies.is_empty(),
        "the run stopped before completing its requests"
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn rate_limited_billing_keeps_charge_keys_and_reads_every_page() {
    let mut script = start(true);
    for use_number in 1..=201 {
        append(&mut script, charged(use_number, 1000), use_number == 119);
    }
    let entries = ledger(201, 1000);
    script.push(reply(
        "GET",
        "/api/v2/contracts/1/payments?limit=200",
        200,
        json!(&entries[..200]),
    ));
    append(
        &mut script,
        reply(
            "GET",
            "/api/v2/contracts/1/payments?limit=200&next=4",
            200,
            json!(&entries[200..]),
        ),
        true,
    );
    let output = run(script, 201, 1000).await;
    assert_eq!(output.matches("doing the work").count(), 201);
    assert!(output.contains("201 uses done; 799 n is still locked"));
    assert!(output.contains("the statement has 201 entries, 201 n billed in total"));
}

#[tokio::test]
async fn a_full_final_page_is_followed_without_counting_locks_or_refunds() {
    let mut script = start(false);
    script.extend((1..=198).map(|use_number| charged(use_number, 1000)));
    script.push(reply(
        "GET",
        "/api/v2/contracts/1/payments?limit=200",
        200,
        json!(ledger(198, 1000)),
    ));
    script.push(reply(
        "GET",
        "/api/v2/contracts/1/payments?limit=200&next=1",
        200,
        json!([]),
    ));
    let output = run(script, 198, 1000).await;
    assert!(output.contains("the statement has 198 entries, 198 n billed in total"));
}

#[tokio::test]
async fn exhausting_the_quota_still_reads_the_completed_charges() {
    let mut script = start(false);
    script.extend((1..=3).map(|use_number| charged(use_number, 3)));
    script.push(Reply {
        status: 409,
        body: json!({"error":"conflict", "error_info":"not_enough_amount"}),
        ..charged(4, 3)
    });
    script.push(reply(
        "GET",
        "/api/v2/contracts/1/payments?limit=200",
        200,
        json!(ledger(3, 3).into_iter().skip(1).collect::<Vec<_>>()),
    ));
    let output = run(script, 5, 3).await;
    assert!(output.contains("3 uses done: the quota is spent"));
    assert!(output.contains("the statement has 3 entries, 3 n billed in total"));
}
