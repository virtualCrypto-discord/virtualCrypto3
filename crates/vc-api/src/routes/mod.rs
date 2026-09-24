pub mod connect;
mod csrf;
pub mod documentation;
pub mod grant_requests;
pub mod grants;
pub mod guild_token;
pub mod idempotency;
mod interaction_receipts;
pub mod interactions;
pub mod limited;
pub mod oauth2;
pub mod oauth2_clients;
pub mod oauth2_token;
pub mod pagination;
pub mod v2;
pub mod web;

use axum::Json;
use axum::Router;
use axum::extract::Request;
use axum::http::header::ACCEPT;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use serde_json::json;
use tower_http::trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use tower::ServiceBuilder;

use crate::state::AppState;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

pub fn router(web_root: std::path::PathBuf) -> Router<AppState> {
    // Everything under `/api` shares the `plug :accepts, ["json"]` pipeline, and
    // the Discord interactions endpoint lives in that scope too.
    let api = Router::new()
        .route(
            "/api/integrations/discord/interactions",
            post(interactions::index),
        )
        .merge(v2::router())
        // The document the site's pages are drawn from. Under `/api` because it
        // is read rather than visited, and because the scope already says what a
        // caller has to accept.
        .merge(documentation::router())
        .layer(middleware::from_fn(require_json_accept));

    Router::new()
        .route("/health", get(health))
        // The two pages a browser visits that are not the SPA's own routes.
        .route("/login", get(web::login))
        .route("/logout", get(web::logout))
        .route("/callback/discord", get(web::discord_callback))
        // Where the old site sent a browser that asked for the bot or the guild.
        .route("/invite", get(web::invite))
        .route("/support", get(web::support))
        .route("/token", post(web::token))
        // The consent screen, which OAuth2 sends browsers to.
        .route("/oauth2/authorize", get(oauth2::authorize))
        .route("/oauth2/authorize", post(oauth2::approve))
        .route("/oauth2/token", post(oauth2_token::token))
        // The collection: what the caller owns, and registration. The list used to be
        // on `/@me` with the read, which is the one path RFC 7592 fixes: registration
        // answers `registration_client_uri: /oauth2/clients/@me`, so the read has to be
        // there and the list cannot be.
        .route(
            "/oauth2/clients",
            get(oauth2_clients::mine).post(oauth2_clients::register),
        )
        .route(
            "/oauth2/clients/@me",
            get(oauth2_clients::me).patch(oauth2_clients::edit),
        )
        // Not under `/oauth2`: this is a call a page makes about an application it
        // names, rather than a registration endpoint about the caller.
        .route("/applications/{id}/connect", post(connect::connect))
        .route("/applications/{id}/grants", get(grants::index))
        .route(
            "/applications/{id}/grants/{guild_id}",
            delete(grants::revoke),
        )
        .route(
            "/oauth2/clients/@me/grant-requests",
            get(grant_requests::index).post(grant_requests::create),
        )
        .route("/oauth2/token/revoke", post(oauth2_token::revoke))
        .merge(api)
        // Vite writes every file it builds under `assets/` with a content hash in
        // its name, so a change to one changes the name: a copy may be kept for as
        // long as anything still asks for that name, which is forever.
        //
        // Claimed by its own route rather than left to the fallback so that the
        // fallback can be told the opposite, below.
        .nest_service(
            "/assets",
            ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                ))
                .service(ServeDir::new(web_root.join("assets"))),
        )
        // Everything nothing else claimed is a client-side route, so the SPA is
        // handed its own index and left to route it.
        //
        // `no-cache` here, and it is not a detail: the index is the file that names
        // the hashed assets, so a cached copy is a browser asking for files the
        // deploy has already removed. The two headers are one decision.
        .fallback_service(
            ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ))
                .service(
                    ServeDir::new(&web_root).fallback(ServeFile::new(web_root.join("index.html"))),
                ),
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
