//! `POST /oauth2/token`, over HTTP.
//!
//! Additions rather than ports: OAuth2 is the part of the migration with no
//! Elixir test to follow.

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use sqlx::PgPool;
use support::{fake, state};
use tower::ServiceExt;

async fn form(app: Router, body: &str) -> (u16, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth2/token")
                .header(
                    axum::http::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(Body::from(body.to_owned()))
                .expect("request"),
        )
        .await
        .expect("router response");

    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");

    (status, serde_json::from_slice(&bytes).expect("a json body"))
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_body_without_a_grant_type_is_told_so(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, body) = form(app, "").await;

    assert_eq!(status, 400);
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(body["error_description"], "grant_type_parameter_missing");
}

/// A grant this endpoint does not have — which, until the refresh and the two
/// credential shapes are written, is all of them but one.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_grant_that_does_not_exist_is_unsupported(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, body) = form(app, "grant_type=password").await;

    assert_eq!(status, 400);
    assert_eq!(body["error"], "unsupported_grant_type");
    assert_eq!(body["error_description"], serde_json::Value::Null);
}

/// The parameter that is missing is the one named, in the Elixir's order.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_missing_parameter_is_named(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (_, body) = form(
        app.clone(),
        "grant_type=authorization_code&redirect_uri=x&code=y",
    )
    .await;
    assert_eq!(body["error_description"], "client_id");

    let (_, body) = form(
        app.clone(),
        "grant_type=authorization_code&client_id=x&code=y",
    )
    .await;
    assert_eq!(body["error_description"], "redirect_uri");

    let (_, body) = form(
        app,
        "grant_type=authorization_code&client_id=x&redirect_uri=y",
    )
    .await;
    assert_eq!(body["error_description"], "code");
}

/// A code that was never issued, over HTTP, is a `400` — the status the Elixir
/// does not set and the specification does.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_code_we_did_not_issue_is_a_bad_request(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, body) = form(
        app,
        "grant_type=authorization_code&client_id=x&redirect_uri=y&code=z",
    )
    .await;

    assert_eq!(status, 400);
    assert_eq!(body["error"], "invalid_grant");
    assert_eq!(body["error_description"], "invalid_code");
}

/// Nothing read an `Authorization` header here until the credentials grants did,
/// and a client without one is refused the same way in both of them.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn credentials_without_basic_auth_are_refused(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, body) = form(app, "grant_type=client_credentials&guild_id=1").await;

    assert_eq!(status, 400);
    assert_eq!(body["error"], "invalid_client");
}

/// An application that does not exist and one whose secret is wrong are one
/// answer, which is what the Elixir's caller ends up giving too.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn credentials_that_do_not_verify_are_refused(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth2/token")
                .header(
                    axum::http::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                // "x:y"
                .header(axum::http::header::AUTHORIZATION, "Basic eDp5")
                .body(Body::from("grant_type=client_credentials&scope=vc.pay"))
                .expect("request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 400);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(body["error"], "invalid_client");
}
