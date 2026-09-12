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
    let jti = Uuid::new_v4();
    let at = PrimitiveDateTime::new(now.date(), now.time());
    let issued = now.unix_timestamp();

    sqlx::query!(
        "INSERT INTO user_access_tokens (user_id, token_id, expires, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)",
        user_id,
        jti,
        at + TTL,
        at
    )
    .execute(pool)
    .await?;

    let claims = Claims {
        sub: user_id.to_string(),
        exp: issued + TTL.whole_seconds(),
        iat: Some(issued),
        nbf: Some(issued),
        iss: ISSUER.to_string(),
        aud: Some(AUDIENCE.to_string()),
        jti: jti.to_string(),
        kind: "user".to_string(),
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        typ: Some("access".to_string()),
    };

    Ok(jwt::sign(&claims, secret)?)
}
