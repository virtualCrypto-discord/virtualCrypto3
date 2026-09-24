//! The Discord interaction handshake, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/common_test.exs`.

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Request};
use serde_json::Value;
use sqlx::PgPool;
use support::{Response, fake, sign_interaction, state};
use tower::ServiceExt;

const URI: &str = "/api/integrations/discord/interactions";

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_command_without_an_interaction_id_is_refused(pool: PgPool) {
    let response = interaction(
        router(pool),
        serde_json::json!({"type":2, "data":{"name":"help"}, "user":{"id":"12"}}),
    )
    .await;
    assert_eq!(response.status, 400);
    assert_eq!(response.body, "Missing or invalid interaction ID");
}

async fn send(app: Router, body: &[u8], signed: bool) -> Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(URI)
        .header("content-type", "application/json");

    if signed {
        let (timestamp, signature) = sign_interaction(body);
        builder = builder
            .header("x-signature-timestamp", timestamp)
            .header("x-signature-ed25519", signature);
    }

    let response = app
        .oneshot(builder.body(Body::from(body.to_vec())).expect("request"))
        .await
        .expect("router response");

    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };

    Response {
        status,
        headers,
        body,
    }
}

async fn interaction(app: Router, payload: Value) -> Response {
    let body = serde_json::to_vec(&payload).expect("encode body");
    send(app, &body, true).await
}

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn without_a_body_or_signature_it_is_401(pool: PgPool) {
    let response = send(router(pool), b"", false).await;

    assert_eq!(response.status, 401);
    assert_eq!(
        response.body,
        Value::String("invalid request signature".into())
    );
}

/// A body that was not signed the way Discord signs is refused, even when it
/// parses.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_forged_signature_is_401(pool: PgPool) {
    let body = br#"{"type":1}"#;
    let request = Request::builder()
        .method("POST")
        .uri(URI)
        .header("content-type", "application/json")
        .header("x-signature-timestamp", "1")
        .header("x-signature-ed25519", "00".repeat(64))
        .body(Body::from(body.to_vec()))
        .expect("request");

    let response = router(pool)
        .oneshot(request)
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 401);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_signed_body_without_a_type_is_400(pool: PgPool) {
    let response = interaction(router(pool), serde_json::json!({})).await;

    assert_eq!(response.status, 400);
    assert_eq!(response.body, Value::String("Type Not Found".into()));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_string_type_is_400(pool: PgPool) {
    let response = interaction(router(pool), serde_json::json!({ "type": "1" })).await;

    assert_eq!(response.status, 400);
    assert_eq!(response.body, Value::String("Type Not Found".into()));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_null_type_is_400(pool: PgPool) {
    let response = interaction(router(pool), serde_json::json!({ "type": null })).await;

    assert_eq!(response.status, 400);
    assert_eq!(response.body, Value::String("Type Not Found".into()));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_ping_is_answered_with_a_pong(pool: PgPool) {
    let response = interaction(router(pool), serde_json::json!({ "type": 1 })).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, serde_json::json!({ "type": 1 }));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_type_is_400(pool: PgPool) {
    let response = interaction(router(pool), serde_json::json!({ "type": -21 })).await;

    assert_eq!(response.status, 400);
    assert_eq!(response.body, Value::String("Type Not Found".into()));
}

/// The signature covers the raw body, so a whitespace change invalidates it even
/// though the JSON is equivalent.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_signature_covers_the_exact_bytes(pool: PgPool) {
    let signed = br#"{"type":1}"#;
    let (timestamp, signature) = sign_interaction(signed);

    let tampered = br#"{"type":1} "#;
    let request = Request::builder()
        .method("POST")
        .uri(URI)
        .header("content-type", "application/json")
        .header("x-signature-timestamp", timestamp)
        .header("x-signature-ed25519", signature)
        .body(Body::from(tampered.to_vec()))
        .expect("request");

    let response = router(pool)
        .oneshot(request)
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 401);
}

/// A header that appears twice is unusable, as it is for
/// `Plug.Conn.get_req_header/2` when it does not match a single-element list.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_duplicated_signature_header_is_401(pool: PgPool) {
    let body = br#"{"type":1}"#;
    let (timestamp, signature) = sign_interaction(body);

    let request = Request::builder()
        .method("POST")
        .uri(URI)
        .header("content-type", "application/json")
        .header("x-signature-timestamp", timestamp)
        .header(
            "x-signature-ed25519",
            HeaderValue::from_str(&signature).expect("header"),
        )
        .header(
            "x-signature-ed25519",
            HeaderValue::from_str(&signature).expect("header"),
        )
        .body(Body::from(body.to_vec()))
        .expect("request");

    let response = router(pool)
        .oneshot(request)
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 401);
}
