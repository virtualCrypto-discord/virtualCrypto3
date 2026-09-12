//! Minting the tokens the service hands out.
//!
//! Until the web login needed one, this existed only inside the test support —
//! which is how it was possible for the service to have no way of issuing a
//! token at all. The support now calls this, so the suite tests the real thing.

use sqlx::PgPool;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use crate::claims::{AUDIENCE, Claims, ISSUER};
use crate::error::AuthError;
use crate::jwt;

/// How long a token lives. Guardian's hour rather than a choice, and the value
/// the Elixir service used.
pub const TTL: Duration = Duration::hours(1);

/// Issue a token for a user, recording it so that it can be revoked.
///
/// Two steps, and the second is the one worth naming: the claims are signed
/// *and* their `jti` is written down. A token whose id is not recorded cannot be
/// revoked, because revocation is a `DELETE` against exactly that row.
pub async fn user_token(
    pool: &PgPool,
    secret: &[u8],
    user_id: i64,
    scopes: &[&str],
    now: OffsetDateTime,
) -> Result<String, AuthError> {
    issue(pool, secret, user_id, "user", scopes, now).await
}

/// `Guardian.issue_token_for_app/2`: a token that says what an application may
/// do, rather than one that says who a user is.
///
/// The `kind` claim is the whole of the difference, and it is not a small one:
/// the API dispatches on it, and one endpoint answers a user and an application
/// differently for the same path.
pub async fn app_token(
    pool: &PgPool,
    secret: &[u8],
    application_user_id: i64,
    scopes: &[&str],
    now: OffsetDateTime,
) -> Result<String, AuthError> {
    issue(pool, secret, application_user_id, "app", scopes, now).await
}

/// The issuance both kinds share.
///
/// Guardian's `after_encode_and_sign/4` writes a `user_access_tokens` row for
/// `kind in ["user", "app"]` — the same table for both — so an application's
/// token is revoked the same way a user's is, by deleting a row.
async fn issue(
    pool: &PgPool,
    secret: &[u8],
    subject: i64,
    kind: &str,
    scopes: &[&str],
    now: OffsetDateTime,
) -> Result<String, AuthError> {
    let jti = Uuid::new_v4();
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();
    let issued = now.unix_timestamp();

    sqlx::query!(
        "INSERT INTO user_access_tokens (user_id, token_id, expires, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)",
        subject,
        jti,
        at + TTL,
        at
    )
    .execute(pool)
    .await?;

    let claims = Claims {
        sub: subject.to_string(),
        exp: issued + TTL.whole_seconds(),
        iat: Some(issued),
        nbf: Some(issued),
        iss: ISSUER.to_string(),
        aud: Some(AUDIENCE.to_string()),
        jti: jti.to_string(),
        kind: kind.to_string(),
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        typ: Some("access".to_string()),
    };

    jwt::sign(&claims, secret)
}

/// The scopes a `client_credentials` token may carry, which are **not** the
/// browser flow's: that one allows `openid` and nothing else, and neither check
/// can be reused for the other.
pub const APP_SCOPES: &[&str] = &["vc.pay", "vc.claim", "oauth2.register"];

/// Whether a `client_credentials` request's scopes are acceptable: no repeats,
/// and nothing outside [`APP_SCOPES`].
pub fn app_scopes_are_valid(scopes: &[&str]) -> bool {
    let unique: std::collections::HashSet<&str> = scopes.iter().copied().collect();

    unique.len() == scopes.len() && scopes.iter().all(|scope| APP_SCOPES.contains(scope))
}

/// `Guardian.revoke_with_jti/1`: forget a JWT by the id it was issued under.
///
/// The row is the only thing that makes a JWT revocable — its signature stays
/// valid until it expires — so this is the JWT half of revocation. It is also the
/// whole of what the Elixir's endpoint knew how to do, which is why a client
/// could revoke an app's token there and not one it had just been issued.
///
/// The `kind` is not consulted: the table does not record one, so both kinds are
/// revoked the same way, by the `jti` alone.
pub async fn revoke_by_jti(pool: &PgPool, jti: &str) -> Result<bool, AuthError> {
    let Ok(jti) = Uuid::parse_str(jti) else {
        return Ok(false);
    };

    let deleted = sqlx::query!("DELETE FROM user_access_tokens WHERE token_id = $1", jti)
        .execute(pool)
        .await?;

    Ok(deleted.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_scopes_are_the_ones_the_credentials_grant_checks() {
        assert!(app_scopes_are_valid(&["vc.pay"]));
        assert!(app_scopes_are_valid(&["vc.pay", "vc.claim"]));
        assert!(app_scopes_are_valid(&[]));
    }

    /// A repeat is refused, as `MapSet.size/1` against the list's length is.
    #[test]
    fn a_repeated_app_scope_is_refused() {
        assert!(!app_scopes_are_valid(&["vc.pay", "vc.pay"]));
    }

    /// And `openid` is not one of them, which is the reason the two checks are
    /// two: the browser flow's set and this one do not overlap.
    #[test]
    fn an_app_scope_that_is_not_one_is_refused() {
        assert!(!app_scopes_are_valid(&["openid"]));
        assert!(!app_scopes_are_valid(&["vc.pay", "root"]));
    }
}
