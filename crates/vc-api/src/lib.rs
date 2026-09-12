//! axum routers implementing the virtualCrypto HTTP API.

pub mod discord;
pub mod error;
pub mod routes;
pub mod state;

use axum::Router;

pub use error::ApiError;
pub use state::AppState;

/// Build the application router.
pub fn router(state: AppState) -> Router {
    routes::router().with_state(state)
}
