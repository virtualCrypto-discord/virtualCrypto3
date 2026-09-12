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
    routes::router().with_state(state)
}
