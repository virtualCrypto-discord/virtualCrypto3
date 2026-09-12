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

    /// 409 `conflict` with an `error_info` such as `invalid_status`.
    #[error("conflict: {0}")]
    Conflict(&'static str),

    /// 400 `invalid_request` / `invalid_metadata` carrying the validation details.
    #[error("invalid metadata")]
    InvalidMetadata(Vec<String>),

    /// 400 `invalid_request` with the metadata-limit message the controller uses.
    #[error("metadata limit reached")]
    MetadataLimit,

    /// 403 `insufficient_scope` / `token_verification_failed`, which is what the
    /// payment endpoints use for a token without `vc.pay` (the claim endpoints
    /// answer `invalid_token` / `permission_denied` instead).
    #[error("insufficient scope")]
    InsufficientScope,

    /// 400 `invalid_request` / `invalid_<tag>_at_<index>` from a bulk body entry.
    #[error("invalid {tag} at {index}")]
    BulkInvalid { tag: &'static str, index: usize },
}

/// The controller's message for [`ApiError::MetadataLimit`]. `error_description`
/// is outside the specification, so this is reproduced for fidelity, not because
/// a client may rely on the wording.
const METADATA_LIMIT_MESSAGE: &str = "The upper limit of the number of metadata is 50, and it is highly possible that this has been reached. (Maybe for other reasons)";

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
            ApiError::Conflict(info) => (
                StatusCode::CONFLICT,
                Json(json!({ "error": "conflict", "error_info": info })),
            )
                .into_response(),
            ApiError::InvalidMetadata(details) => (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_request",
                    "error_description": "invalid_metadata",
                    "error_description_details": details,
                })),
            )
                .into_response(),
            ApiError::MetadataLimit => (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_request",
                    "error_description": METADATA_LIMIT_MESSAGE,
                })),
            )
                .into_response(),
            ApiError::InsufficientScope => (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": "insufficient_scope",
                    "error_description": "token_verification_failed",
                })),
            )
                .into_response(),
            ApiError::BulkInvalid { tag, index } => (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_request",
                    "error_description": format!("invalid_{tag}_at_{index}"),
                })),
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
