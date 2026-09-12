pub mod interactions;
pub mod v2;
pub mod web;

use axum::Json;
use axum::Router;
use axum::extract::Request;
use axum::http::header::ACCEPT;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;
use tower_http::trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::state::AppState;
use tower_http::services::{ServeDir, ServeFile};

pub fn router(web_root: std::path::PathBuf) -> Router<AppState> {
    // Everything under `/api` shares the `plug :accepts, ["json"]` pipeline, and
    // the Discord interactions endpoint lives in that scope too.
    let api = Router::new()
        .route(
            "/api/integrations/discord/interactions",
            post(interactions::index),
        )
        .merge(v2::router())
        .layer(middleware::from_fn(require_json_accept));

    Router::new()
        .route("/health", get(health))
        // The two pages a browser visits that are not the SPA's own routes.
        .route("/login", get(web::login))
        .route("/logout", get(web::logout))
        .route("/callback/discord", get(web::discord_callback))
        .merge(api)
        // Everything nothing else claimed is a client-side route, so the SPA is
        // handed its own index and left to route it.
        .fallback_service(
            ServeDir::new(&web_root).fallback(ServeFile::new(web_root.join("index.html"))),
        )
        .layer(
            // Discord only says "the endpoint URL could not be validated", so a
            // request log is what makes the handshake debuggable.
            TraceLayer::new_for_http()
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": vc_core::version(),
    }))
}

/// Mirrors Phoenix's `plug :accepts, ["json"]`, which the router applies to the
/// whole `/api` scope. A request whose `Accept` header cannot be satisfied with
/// JSON is refused with 406 before reaching any handler.
async fn require_json_accept(request: Request, next: Next) -> Response {
    if accepts_json(request.headers()) {
        next.run(request).await
    } else {
        (
            StatusCode::NOT_ACCEPTABLE,
            Json(json!({ "errors": { "detail": "Not Acceptable" } })),
        )
            .into_response()
    }
}

/// A missing `Accept` header is treated as `*/*`, which Phoenix does too. Media
/// type parameters are ignored, so a `q=0` exclusion is not honoured.
fn accepts_json(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(ACCEPT) else {
        return true;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    if value.trim().is_empty() {
        return true;
    }

    value.split(',').any(|entry| {
        let media = entry
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();

        matches!(media.as_str(), "*/*" | "application/*" | "application/json")
    })
}
