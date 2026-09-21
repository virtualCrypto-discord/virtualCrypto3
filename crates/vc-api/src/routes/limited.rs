//! An authenticated caller, within their allowance.

use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use vc_auth::{AuthUser, Kind};

use crate::state::AppState;
use vc_core::grant::Target;

/// An authenticated caller, counted against their own allowance, and the
/// currencies the token is for.
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
///
/// Two kinds of credential arrive here, and both are the account acting: the
/// account's own JWT — a browser session, a personal access token, an
/// application's own `client_credentials` token — and a **grant's** token, which
/// is the UUID of an `access_tokens` row a person approved. The second is a
/// delegation, so it is narrowed to the currencies the grant named; the first is
/// the account itself and is not, which is the whole of the difference
/// `docs/resources.md` draws. [`Self::resources`] is that narrowing: empty is
/// every currency, which is both a grant that named none and every credential
/// that is the account's own.
pub struct Limited {
    user: AuthUser,
    resources: Vec<i64>,
    delegated: bool,
}

impl Limited {
    /// Whether this credential acts under a grant rather than as the account itself.
    pub fn is_delegated(&self) -> bool {
        self.delegated
    }

    /// The currencies the token is for. Empty is every currency — the empty set a
    /// grant written before resources existed carries, the collection form of
    /// `resource`, and every credential that is not a grant's at all.
    pub fn resources(&self) -> &[i64] {
        &self.resources
    }
}

/// So that switching a handler over is a change to its signature and nothing else:
/// `user.scopes` and `user.subject` keep working, and only the type says that this
/// caller has been counted.
impl std::ops::Deref for Limited {
    type Target = AuthUser;

    fn deref(&self) -> &AuthUser {
        &self.user
    }
}

impl FromRequestParts<AppState> for Limited {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // A grant token is an `access_tokens` row's own id, which is a UUID and
        // never a JWT — the two cannot be confused, so which path to take is
        // decided by the string rather than by trying one and falling back.
        let grant = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(vc_auth::extractor::bearer_token)
            .and_then(|token| Uuid::parse_str(token).ok());

        let (user, resources) = match grant {
            Some(token_id) => {
                let resolved = vc_core::grant::resolve_token(
                    state.pool(),
                    token_id,
                    OffsetDateTime::now_utc(),
                )
                .await
                .map_err(|error| crate::error::ApiError::from(error).into_response())?;

                // A guild's grant is the guild issuing endpoint's, and this is not
                // it: a token written for one is refused the way an unknown token
                // is, which is what keeps the two kinds apart.
                let Some(resolved) =
                    resolved.filter(|grant| matches!(grant.target, Target::User(_)))
                else {
                    return Err(unauthorized());
                };

                (
                    AuthUser {
                        subject: i64::from(resolved.account_id),
                        kind: Kind::User,
                        scopes: vc_auth::Scopes::from_list(&resolved.scopes),
                        jti: token_id.to_string(),
                    },
                    resolved.resources,
                )
            }
            None => (
                AuthUser::from_request_parts(parts, state)
                    .await
                    .map_err(IntoResponse::into_response)?,
                Vec::new(),
            ),
        };

        if !state.limiter().allow(&format!("v2:{}", user.subject)) {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "rate_limited" })),
            )
                .into_response());
        }

        Ok(Limited {
            user,
            resources,
            delegated: grant.is_some(),
        })
    }
}

/// The answer to a token that is not one this extractor accepts, or is a grant's
/// token for a guild rather than a person. One answer for all of them, the way
/// `AuthUser` gives one: a caller learns that this token does not work here.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "invalid_token" })),
    )
        .into_response()
}
