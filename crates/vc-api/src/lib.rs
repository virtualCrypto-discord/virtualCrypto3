//! axum routers implementing the virtualCrypto HTTP API.

include!(concat!(env!("OUT_DIR"), "/messages.rs"));

pub mod claim_list;
pub mod command;
pub mod components;
pub mod custom_id;
pub mod developer;
pub mod discord;
pub mod discord_auth;
pub mod discord_commands;
mod discord_id;
pub mod docs;
pub mod error;
mod json_number;
pub mod notification;
pub mod permissions;
pub mod rate_limit;
pub mod resource;
pub mod routes;
pub mod scheduler;
pub mod security;
pub mod state;

use axum::Router;

pub use error::ApiError;
pub use state::AppState;

/// Build the application router.
pub fn router(state: AppState) -> Router {
    router_with_web(state, default_web_root())
}

/// Where the built SPA is, which is where Vite puts it unless told otherwise.
/// A deployment sets `WEB_ROOT` to wherever it unpacked the assets.
pub fn default_web_root() -> std::path::PathBuf {
    std::env::var("WEB_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("web/dist"))
}

/// Build the router serving the SPA at `web_root`.
///
/// The SPA is the *fallback*, not a mount: `/api` and `/health` keep their
/// routes, and only what nothing else claims is treated as a client-side route.
///
/// The behaviour-counting middleware is layered here, over everything, because
/// it needs the state its counters belong to — and because a warning about the
/// *service's* error rate must see every route's responses, fallback included.
pub fn router_with_web(state: AppState, web_root: std::path::PathBuf) -> Router {
    routes::router(web_root)
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state,
            routes::record_behaviour,
        ))
}
