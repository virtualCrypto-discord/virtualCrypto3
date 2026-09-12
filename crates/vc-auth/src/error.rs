use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    /// No usable `Authorization: Bearer` header. The Elixir plug reports this as
    /// `invalid_request` with status 400 rather than 401.
    #[error("missing authorization header")]
    Missing,
    /// A header was present but the token is unusable. Covers a bad signature,
    /// a bad issuer/audience, an unknown `kind`, an unparsable `sub`/`jti`, and a
    /// `jti` that has no row in `user_access_tokens`.
    #[error("invalid token")]
    Invalid,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl From<jsonwebtoken::errors::Error> for AuthError {
    fn from(_: jsonwebtoken::errors::Error) -> Self {
        AuthError::Invalid
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        match self {
            AuthError::Missing => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_request" })),
            )
                .into_response(),
            AuthError::Invalid => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid_token" })),
            )
                .into_response(),
            AuthError::Database(error) => {
                tracing::error!(%error, "authentication failed while reading the token store");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "errors": { "detail": "Internal Server Error" } })),
                )
                    .into_response()
            }
        }
    }
}
