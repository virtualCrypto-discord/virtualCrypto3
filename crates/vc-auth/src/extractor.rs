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

/// Read a Bearer credential without changing the case-sensitive token itself.
/// Authentication scheme names are case-insensitive (RFC 9110 section 11.1).
pub fn bearer_token(header: &str) -> Option<&str> {
    let prefix = "Bearer ";
    if !header.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
        return None;
    }
    header.get(prefix.len()..).filter(|token| !token.is_empty())
}

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

        let token = bearer_token(header).ok_or(AuthError::Missing)?;

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

#[cfg(test)]
mod tests {
    use super::bearer_token;

    #[test]
    fn scheme_case_does_not_change_the_token() {
        for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
            assert_eq!(
                bearer_token(&format!("{scheme} AbC.dEf-X_y")),
                Some("AbC.dEf-X_y")
            );
        }
    }

    #[test]
    fn other_schemes_and_missing_credentials_are_rejected() {
        for header in [
            "",
            "Bearer",
            "Bearer ",
            "Basic token",
            "Bearertoken",
            "BearerX token",
            "💰💰 token",
        ] {
            assert_eq!(bearer_token(header), None, "{header}");
        }
    }
}
