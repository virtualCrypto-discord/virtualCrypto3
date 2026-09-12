//! axum routers implementing the virtualCrypto HTTP API.

pub mod claim_list;
pub mod command;
pub mod custom_id;
pub mod discord;
pub mod error;
pub mod rate_limit;
pub mod routes;
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
pub fn router_with_web(state: AppState, web_root: std::path::PathBuf) -> Router {
    routes::router(web_root).with_state(state)
}
