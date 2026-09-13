//! Serving the built SPA.
//!
//! The SPA is the router's fallback, so what matters is that it stays a
//! fallback: the API keeps its own routes, and only what nothing else claims is
//! treated as a client-side route.

mod support;

use std::path::PathBuf;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use sqlx::PgPool;
use support::{fake, state};
use tower::ServiceExt;

/// A directory holding a built SPA, which is all `ServeDir` needs of one.
fn built_spa() -> PathBuf {
    let root = std::env::temp_dir().join(format!("vc-web-{}", std::process::id()));
    std::fs::create_dir_all(root.join("assets")).expect("the fixture directory");
    std::fs::write(root.join("index.html"), "<!doctype html><title>spa</title>").expect("index");
    std::fs::write(root.join("assets/app.js"), "console.log(1)").expect("an asset");

    root
}

async fn fetch(app: Router, uri: &str) -> (u16, String) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");

    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_asset_is_served_as_it_is(pool: PgPool) {
    let app = vc_api::router_with_web(state(pool, fake()), built_spa());

    let (status, body) = fetch(app, "/assets/app.js").await;

    assert_eq!(status, 200);
    assert_eq!(body, "console.log(1)");
}

/// A path no route claims is the SPA's to route, so it gets the index.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_client_route_is_answered_with_the_index(pool: PgPool) {
    let app = vc_api::router_with_web(state(pool, fake()), built_spa());

    let (status, body) = fetch(app, "/me/applications").await;

    assert_eq!(status, 200);
    assert!(body.contains("<title>spa</title>"), "body: {body}");
}

/// The fallback must not swallow the API, which is the whole risk of serving a
/// SPA from the same origin.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_api_keeps_its_own_routes(pool: PgPool) {
    let app = vc_api::router_with_web(state(pool, fake()), built_spa());

    let (status, body) = fetch(app, "/health").await;

    assert_eq!(status, 200);
    assert!(body.contains("\"status\":\"ok\""), "body: {body}");
}

/// The `Cache-Control` a request is answered with. These two headers are one
/// decision — names that change may be kept, the file that names them may not — so
/// they are tested next to each other.
async fn cache_control(app: Router, uri: &str) -> Option<String> {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    response
        .headers()
        .get(axum::http::header::CACHE_CONTROL)
        .map(|value| value.to_str().expect("a header value").to_owned())
}

/// A hashed name may be kept for as long as anything asks for it: if the file
/// changes, so does its name, so this copy is never the wrong one.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_hashed_asset_may_be_kept_forever(pool: PgPool) {
    let app = vc_api::router_with_web(state(pool, fake()), built_spa());

    assert_eq!(
        cache_control(app, "/assets/app.js").await.as_deref(),
        Some("public, max-age=31536000, immutable")
    );
}

/// The index may not be kept at all: it is what names the hashed assets, so a stale
/// copy sends a browser looking for files the deploy has already removed. This is
/// the half of the pair that breaks in a way nobody reports as a bug — the page
/// merely fails to load for people who visited before.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_index_may_not_be_kept(pool: PgPool) {
    let app = vc_api::router_with_web(state(pool, fake()), built_spa());

    assert_eq!(
        cache_control(app, "/me/applications").await.as_deref(),
        Some("no-cache")
    );
}
