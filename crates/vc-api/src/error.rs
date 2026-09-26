use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
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

    #[error("request timed out")]
    RequestTimeout,

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

    /// 400 `invalid_target`: RFC 8707's own error, for a `resource` that is not a
    /// resource of this service — not absolute, not ours, not a currency, or a
    /// currency of a guild the ask was not put to. It is a 400 rather than a 403
    /// because the *request* is wrong about what it is asking for: nothing about
    /// the caller has been found wanting.
    #[error("invalid target")]
    InvalidTarget,

    /// 400 `invalid_request` / `invalid_<tag>_at_<index>` from a bulk body entry.
    #[error("invalid {tag} at {index}")]
    BulkInvalid { tag: &'static str, index: usize },
}

/// The controller's message for [`ApiError::MetadataLimit`]. `error_description`
/// is outside the specification, so this is reproduced for fidelity, not because
/// a client may rely on the wording.
const METADATA_LIMIT_MESSAGE: &str = "The upper limit of the number of metadata is 50, and it is highly possible that this has been reached. (Maybe for other reasons)";

impl ApiError {
    /// The status and the body this error answers with, as values rather than as
    /// a finished response.
    ///
    /// [`IntoResponse`] is written in terms of this, and so is the one caller
    /// that has to **store** what it answered: the idempotency layer replays a
    /// refusal, and a refusal it cannot take apart is one it cannot store. Doing
    /// it this way rather than by hand is what keeps the two from drifting apart.
    pub fn parts(self) -> (StatusCode, Value) {
        match self {
            ApiError::RequestTimeout => (
                StatusCode::GATEWAY_TIMEOUT,
                json!({ "error": "request_timeout" }),
            ),
            ApiError::PermissionDenied => (
                StatusCode::FORBIDDEN,
                json!({ "error": "invalid_token", "error_description": "permission_denied" }),
            ),
            ApiError::Forbidden(description) => (
                StatusCode::FORBIDDEN,
                json!({ "error": "forbidden", "error_description": description }),
            ),
            ApiError::NotFound => (
                StatusCode::NOT_FOUND,
                json!({ "error": "not_found", "error_description": "not_found" }),
            ),
            ApiError::InvalidRequest(description) => (
                StatusCode::BAD_REQUEST,
                json!({ "error": "invalid_request", "error_description": description }),
            ),
            ApiError::Conflict(info) => (
                StatusCode::CONFLICT,
                json!({ "error": "conflict", "error_info": info }),
            ),
            ApiError::InvalidMetadata(details) => (
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "invalid_request",
                    "error_description": "invalid_metadata",
                    "error_description_details": details,
                }),
            ),
            ApiError::MetadataLimit => (
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "invalid_request",
                    "error_description": METADATA_LIMIT_MESSAGE,
                }),
            ),
            ApiError::InsufficientScope => (
                StatusCode::FORBIDDEN,
                json!({
                    "error": "insufficient_scope",
                    "error_description": "token_verification_failed",
                }),
            ),
            ApiError::InvalidTarget => (
                StatusCode::BAD_REQUEST,
                json!({ "error": "invalid_target" }),
            ),
            ApiError::BulkInvalid { tag, index } => (
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "invalid_request",
                    "error_description": format!("invalid_{tag}_at_{index}"),
                }),
            ),
            ApiError::Core(vc_core::Error::UserNotFound(_))
            | ApiError::Core(vc_core::Error::DiscordAuthNotFound(_))
            | ApiError::Internal(_)
            | ApiError::Discord(_) => {
                tracing::error!(%self, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "errors": { "detail": "Internal Server Error" } }),
                )
            }
            ApiError::Core(vc_core::Error::Database(error)) => {
                if error
                    .as_database_error()
                    .and_then(|error| error.code())
                    .as_deref()
                    == Some("57014")
                {
                    return ApiError::RequestTimeout.parts();
                }
                tracing::error!(%error, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "errors": { "detail": "Internal Server Error" } }),
                )
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = self.parts();

        (status, Json(body)).into_response()
    }
}
