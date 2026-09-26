//! Public redirects; browser identity is established per OAuth authorization.
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

pub async fn invite(State(state): State<AppState>) -> Response {
    Redirect::temporary(&state.links().invite_url).into_response()
}
pub async fn support(State(state): State<AppState>) -> Response {
    Redirect::temporary(&state.links().support_guild_invite_url).into_response()
}

/// Explicitly retired instead of serving the SPA fallback for old clients.
pub async fn retired() -> StatusCode {
    StatusCode::GONE
}
