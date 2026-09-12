use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use uuid::Uuid;

use crate::claims::{Kind, Scopes};
use crate::error::AuthError;
use crate::jwt;
use crate::state::AuthState;

/// The authenticated principal, equivalent to Guardian's loaded resource.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub subject: i64,
    pub kind: Kind,
    pub scopes: Scopes,
    pub jti: String,
}

const BEARER: &str = "Bearer ";

impl<S> FromRequestParts<S> for AuthUser
where
    S: AuthState,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(AuthError::Missing)?;

        let token = header.strip_prefix(BEARER).ok_or(AuthError::Missing)?;
        if token.is_empty() {
            return Err(AuthError::Missing);
        }

        let claims = jwt::verify(token, state.jwt_secret())?;
        let kind = Kind::parse(&claims.kind).ok_or(AuthError::Invalid)?;

        // Guardian's verify_claims/2 only checks that a row exists for the jti;
        // it deliberately does not compare `expires` to the current time.
        // Expired rows are removed by the periodic purge job instead.
        let jti = Uuid::parse_str(&claims.jti).map_err(|_| AuthError::Invalid)?;
        let known = sqlx::query_scalar!(
            "SELECT EXISTS(SELECT 1 FROM user_access_tokens WHERE token_id = $1)",
            jti
        )
        .fetch_one(state.pool())
        .await?;

        if !known.unwrap_or(false) {
            return Err(AuthError::Invalid);
        }

        let subject = claims.sub.parse().map_err(|_| AuthError::Invalid)?;

        Ok(AuthUser {
            subject,
            kind,
            scopes: Scopes::from_list(&claims.scopes),
            jti: claims.jti,
        })
    }
}
