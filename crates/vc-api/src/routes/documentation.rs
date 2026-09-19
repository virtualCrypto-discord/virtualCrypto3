//! The document the site's `/document/*` pages are drawn from.
//!
//! One read, and one document: the pages are a handful of short ones and the SPA
//! routes between them itself, so a read per page would be a read per click of a
//! page that is already in the browser. [`crate::docs`] is where the content
//! comes from, and it is the same content `/help` renders — which is why the two
//! cannot say different things.
//!
//! It lives under `/api` and is read through the same middleware as the rest of
//! the scope: a caller that cannot accept JSON is refused there.

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/docs", get(index))
}

/// Everything: the pages, the commands, the endpoints, with this deployment's
/// addresses already in the links.
async fn index(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(crate::docs::json::document(state.links()))
}
