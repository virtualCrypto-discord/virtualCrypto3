//! axum routers implementing the virtualCrypto HTTP API.

use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

/// Root router. The `/api/v2` surface is mounted here as it is implemented.
pub fn router() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": vc_core::version() }))
}
