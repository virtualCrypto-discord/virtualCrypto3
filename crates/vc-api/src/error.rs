use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use thiserror::Error;

use crate::discord::DiscordError;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error(transparent)]
    Core(#[from] vc_core::Error),

    #[error("discord api error: {0}")]
    Discord(#[from] DiscordError),

    /// The Elixir controllers pattern-match on values they assume exist (for
    /// example `user.discord_id`, `discord_auth.token`) and raise otherwise,
    /// which Phoenix turns into a 500. Keep that mapping.
    #[error("inconsistent state: {0}")]
    Internal(String),

    /// 403 `invalid_token` / `permission_denied`, what Guardian's scope checks
    /// produce inside the claim controller.
    #[error("permission denied")]
    PermissionDenied,

    /// 403 `forbidden` with a description such as `not_related_user` or
    /// `invalid_operator`.
    #[error("forbidden: {0}")]
    Forbidden(&'static str),

    /// 404 `not_found`.
    #[error("not found")]
    NotFound,

    /// 400 `invalid_request` with one of the controller's error codes.
    #[error("invalid request: {0}")]
    InvalidRequest(&'static str),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::PermissionDenied => (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "invalid_token", "error_description": "permission_denied" })),
            )
                .into_response(),
            ApiError::Forbidden(description) => (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "forbidden", "error_description": description })),
            )
                .into_response(),
            ApiError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "error_description": "not_found" })),
            )
                .into_response(),
            ApiError::InvalidRequest(description) => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_request", "error_description": description })),
            )
                .into_response(),
            ApiError::Core(vc_core::Error::UserNotFound(_))
            | ApiError::Core(vc_core::Error::DiscordAuthNotFound(_))
            | ApiError::Internal(_)
            | ApiError::Discord(_) => {
                tracing::error!(%self, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "errors": { "detail": "Internal Server Error" } })),
                )
                    .into_response()
            }
            ApiError::Core(vc_core::Error::Database(error)) => {
                tracing::error!(%error, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "errors": { "detail": "Internal Server Error" } })),
                )
                    .into_response()
            }
        }
    }
}
