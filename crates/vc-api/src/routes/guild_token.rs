//! The guild token: what a guild handed an application, recognised.
//!
//! It is not a JWT, and that is not an oversight. The code flow and
//! `client_credentials` with a `guild_id` have always answered with an
//! `access_tokens` row's own id, and what a guild granted lives in `grant_scopes`
//! beside the grant that row belongs to. `Authz.md` calls `guild` a kind of access
//! token; here the kind is what the token resolves to, and this is the one place
//! that resolves it.
//!
//! So this is the second way a bearer token can mean something, next to
//! `vc_auth::AuthUser`'s JWTs — and it is deliberately a different extractor
//! rather than a branch inside that one: a handler that wants a guild's authority
//! says so in its signature, and a user or application token cannot arrive in its
//! place.

use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use vc_auth::Scopes;

use crate::error::ApiError;
use crate::state::AppState;

/// The authenticated guild: the application a guild allowed, and that guild.
#[derive(Debug, Clone)]
pub struct GuildToken {
    pub application_id: i64,
    /// The application's own account, which an idempotent request is filed under.
    pub account_id: i32,
    pub guild_id: i64,
    pub scopes: Scopes,
}

impl FromRequestParts<AppState> for GuildToken {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(unauthorized)?;

        let token = vc_auth::extractor::bearer_token(header).ok_or_else(unauthorized)?;

        // Anything that is not a UUID is not a grant token, rather than a database
        // error: the column is a uuid.
        let Ok(token_id) = Uuid::parse_str(token) else {
            return Err(unauthorized());
        };

        let resolved =
            vc_core::grant::resolve_token(state.pool(), token_id, OffsetDateTime::now_utc())
                .await
                .map_err(|error| ApiError::from(error).into_response())?;

        let Some(resolved) = resolved else {
            return Err(unauthorized());
        };

        // Counted per application, which is the identity a guild token has: one
        // application's tokens cannot spend another's allowance.
        if !state
            .limiter()
            .allow(&format!("guild:{}", resolved.application_id))
        {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "rate_limited" })),
            )
                .into_response());
        }

        Ok(GuildToken {
            application_id: resolved.application_id,
            account_id: resolved.account_id,
            guild_id: resolved.guild_id,
            scopes: Scopes::from_list(&resolved.scopes),
        })
    }
}

/// The answer to a token that is not a guild token — missing, malformed, expired,
/// revoked, or issued for a guild nobody granted anything in. One answer for all
/// of them, which is what `AuthUser` gives too: a caller learns that this token
/// does not work here, and not which part of it failed.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "invalid_token" })),
    )
        .into_response()
}
