//! An authenticated caller, within their allowance.

use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use vc_auth::AuthUser;

use crate::state::AppState;

/// An authenticated caller, counted against their own allowance.
///
/// **This is an addition.** The Elixir limits the interaction endpoint and nothing
/// else — the v2 API it does not limit at all — so there is no golden for this and
/// the shape below is a choice rather than a reproduction. It is the shape the v2
/// errors already use, which is the only thing it can be faithful to.
///
/// The limit is per account, which is why it is checked here rather than in a
/// layer: an account is only known once the token has been verified, and a layer
/// would have to verify tokens a second time, or count addresses instead. A handler
/// that takes this instead of `AuthUser` cannot forget to be limited.
pub struct Limited(pub AuthUser);

/// So that switching a handler over is a change to its signature and nothing else:
/// `user.scopes` and `user.subject` keep working, and only the type says that this
/// caller has been counted.
impl std::ops::Deref for Limited {
    type Target = AuthUser;

    fn deref(&self) -> &AuthUser {
        &self.0
    }
}

impl FromRequestParts<AppState> for Limited {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;

        if !state.limiter().allow(&format!("v2:{}", user.subject)) {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "rate_limited" })),
            )
                .into_response());
        }

        Ok(Limited(user))
    }
}
