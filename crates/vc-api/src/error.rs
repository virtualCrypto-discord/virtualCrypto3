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
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
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
