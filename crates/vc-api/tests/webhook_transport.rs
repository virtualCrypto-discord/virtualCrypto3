use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use ed25519_dalek::{Signature, SigningKey, Verifier};
use serde_json::{Value, json};
use vc_api::notification::{Direct, Handshake, verify};

#[tokio::test]
async fn json_webhook_accepts_signed_ping() {
    let seed = [9u8; 32];
    let public = SigningKey::from_bytes(&seed).verifying_key();
    let app = Router::new().route(
        "/",
        post(
            move |headers: HeaderMap, Json(body): Json<Value>| async move {
                let signature = headers["X-Signature-Ed25519"].to_str().unwrap();
                let signature: Vec<u8> = (0..signature.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&signature[i..i + 2], 16).unwrap())
                    .collect();
                let signature = Signature::from_slice(&signature).unwrap();
                let timestamp = headers["X-Signature-Timestamp"].to_str().unwrap();
                let message = format!("{timestamp}{}", serde_json::to_string(&body).unwrap());
                if public.verify(message.as_bytes(), &signature).is_err() {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                Json(json!({"type": 1})).into_response()
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let result = verify(&Direct::default(), &url, &seed, 1700000000).await;
    server.abort();
    assert_eq!(
        result,
        Handshake::Passed,
        "a JSON webhook must receive a JSON Content-Type"
    );
}

#[tokio::test]
async fn unresponsive_webhook_finishes_with_failure() {
    let app = Router::new().route(
        "/",
        post(|| async { std::future::pending::<Response>().await }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(12),
        verify(&Direct::default(), &url, &[9u8; 32], 1700000000),
    )
    .await;
    server.abort();
    assert_eq!(
        result.expect("the handshake must finish within 12 seconds"),
        Handshake::Unreachable
    );
}

#[tokio::test]
async fn stalled_response_body_times_out() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        // Wait until the client starts its request, then stall the response body.
        let mut first_byte = [0u8; 1];
        connection.read_exact(&mut first_byte).await.unwrap();
        connection.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{").await.unwrap();
        std::future::pending::<()>().await;
    });
    let delivery = vc_api::notification::delivery(
        &json!({"type": 2, "data": []}),
        &[9u8; 32],
        &url,
        1700000000,
    );
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(7),
        vc_api::notification::send(&Direct::default(), &delivery),
    )
    .await;
    server.abort();
    let error = result
        .expect("the response body must have a deadline")
        .unwrap_err();
    assert!(error.is_timeout(), "{error}");
}

/// Test known-length and chunked responses, including a server that never ends
/// its oversized body. The client must stop at the limit, not wait for EOF.
#[tokio::test]
async fn oversized_webhook_bodies_are_stopped_before_eof() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for chunked in [false, true] {
        for status in [200, 400] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut connection, _) = listener.accept().await.unwrap();
                let mut first_byte = [0u8; 1];
                connection.read_exact(&mut first_byte).await.unwrap();
                let framing = if chunked {
                    "Transfer-Encoding: chunked"
                } else {
                    "Content-Length: 8388608"
                };
                let headers = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{framing}\r\n\r\n"
                );
                connection.write_all(headers.as_bytes()).await.unwrap();
                if chunked {
                    // Individually small chunks must still count towards one limit.
                    let chunk = format!("4000\r\n{}\r\n", " ".repeat(16 * 1024));
                    for _ in 0..5 {
                        if connection.write_all(chunk.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
                std::future::pending::<()>().await;
            });
            let delivery =
                vc_api::notification::delivery(&json!({"type": 1}), &[9; 32], &url, 1700000000);
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                vc_api::notification::send(&Direct::default(), &delivery),
            )
            .await;
            server.abort();
            let (actual_status, body) = result
                .expect("must reject before the five-second delivery timeout")
                .unwrap()
                .unwrap();
            assert_eq!(actual_status, status);
            assert_eq!(body, Value::Null);
            assert_ne!(
                vc_api::notification::handshake(
                    Some((actual_status, &body)),
                    Some((401, &Value::Null))
                ),
                Handshake::Passed
            );
        }
    }
}

#[tokio::test]
async fn webhook_json_at_the_limit_is_accepted_and_one_byte_more_is_rejected() {
    const LIMIT: usize = 64 * 1024;
    for size in [LIMIT, LIMIT + 1] {
        let app = Router::new().route(
            "/",
            post(move || async move {
                let mut body = String::from("{\"type\":1}");
                body.extend(std::iter::repeat_n(' ', size - body.len()));
                body
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let delivery =
            vc_api::notification::delivery(&json!({"type": 1}), &[9; 32], &url, 1700000000);
        let (_, body) = vc_api::notification::send(&Direct::default(), &delivery)
            .await
            .unwrap()
            .unwrap();
        server.abort();
        assert_eq!(
            body,
            if size == LIMIT {
                json!({"type": 1})
            } else {
                Value::Null
            }
        );
    }
}
