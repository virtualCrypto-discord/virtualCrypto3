//! Minting the tokens the service hands out.
//!
//! Until the web login needed one, this existed only inside the test support —
//! which is how it was possible for the service to have no way of issuing a
//! token at all. The support now calls this, so the suite tests the real thing.

use sqlx::PgPool;
use thiserror::Error;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use crate::claims::{AUDIENCE, Claims, ISSUER};
use crate::error::AuthError;
use crate::jwt;

/// How long a token lives. Guardian's hour rather than a choice, and the value
/// the Elixir service used.
pub const TTL: Duration = Duration::hours(1);

/// How long a personal access token lives: a year, which is the point of it. It is the
/// credential something that is not a browser keeps, and one that has to be re-made every hour is
/// not one — the browser can re-make its token because it holds the session; a tool cannot.
pub const PERSONAL_TTL: Duration = Duration::days(365);

/// The scopes an account's own token carries, whether it was issued to a browser session or to a
/// personal access token. One list rather than two: a token for a tool is not a new kind of
/// authority, it is the same account with a longer memory, and a second list would be a second
/// place for the two to drift apart.
pub const BROWSER_SCOPES: &[&str] = &["oauth2.register", "vc.pay", "vc.claim"];

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
    issue(
        pool,
        secret,
        Issuance {
            subject: user_id,
            kind: "user",
            scopes,
            ttl: TTL,
            name: None,
        },
        now,
    )
    .await
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
    issue(
        pool,
        secret,
        Issuance {
            subject: application_user_id,
            kind: "app",
            scopes,
            ttl: TTL,
            name: None,
        },
        now,
    )
    .await
}

/// What every issuance is made of: the row it writes and the claims it signs.
pub struct Issuance<'a> {
    pub subject: i64,
    pub kind: &'a str,
    pub scopes: &'a [&'a str],
    pub ttl: Duration,
    /// Set for a personal access token and nothing else. It is what tells such a row apart from
    /// the hour-long one a session writes, and the name a person revokes it by.
    pub name: Option<&'a str>,
}

/// Why a personal access token could not be issued.
#[derive(Debug, Error)]
pub enum PersonalError {
    /// A token with this name already exists on this account. Names are how one is revoked, so
    /// two of them could not be told apart.
    #[error("a token named {0} already exists")]
    NameTaken(String),
    #[error(transparent)]
    Auth(#[from] AuthError),
}

/// A token for something that is not a browser: the same account, the same scopes as a session's
/// token, a name to revoke it by, and a year instead of an hour.
///
/// The name is unique per account, and the index that says so is what refuses a repeat — a
/// lookup first would be a race with itself, and the caller would still have to handle the index.
pub async fn personal_token(
    pool: &PgPool,
    secret: &[u8],
    user_id: i64,
    name: &str,
    now: OffsetDateTime,
) -> Result<String, PersonalError> {
    let issued = issue(
        pool,
        secret,
        Issuance {
            subject: user_id,
            kind: "user",
            scopes: BROWSER_SCOPES,
            ttl: PERSONAL_TTL,
            name: Some(name),
        },
        now,
    )
    .await;

    match issued {
        Err(AuthError::Database(error)) if unique_violation(&error) => {
            Err(PersonalError::NameTaken(name.to_string()))
        }
        other => Ok(other?),
    }
}

/// Whether Postgres refused a write because it would repeat a unique key: `23505`, which is the
/// name index above and nothing else this table writes.
fn unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|error| error.code().as_deref() == Some("23505"))
}

/// One personal access token, without the token itself: the row does not hold it, and a list is
/// for recognising a credential rather than for reading it back.
pub struct PersonalToken {
    pub name: String,
    /// The column is nullable because the schema is the Elixir's; every row this service writes
    /// has one, and `expires!` says so rather than making every caller unwrap.
    pub expires: PrimitiveDateTime,
}

/// The tokens this account has given a name to, by name.
///
/// Ordered by name rather than by age, because that is how a person finds the one they mean, and
/// because it does not move as new tokens are made.
pub async fn personal_tokens(pool: &PgPool, user_id: i64) -> Result<Vec<PersonalToken>, AuthError> {
    let rows = sqlx::query!(
        r#"SELECT name AS "name!", expires AS "expires!"
           FROM user_access_tokens
           WHERE user_id = $1 AND name IS NOT NULL
           ORDER BY name"#,
        user_id
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| PersonalToken {
            name: row.name,
            expires: row.expires,
        })
        .collect())
}

/// Forgetting a personal access token by the name it was made under: the same deletion the `jti`
/// one performs, found the way a person refers to it.
pub async fn revoke_personal(pool: &PgPool, user_id: i64, name: &str) -> Result<bool, AuthError> {
    let deleted = sqlx::query!(
        "DELETE FROM user_access_tokens WHERE user_id = $1 AND name = $2",
        user_id,
        name
    )
    .execute(pool)
    .await?;

    Ok(deleted.rows_affected() > 0)
}

/// The issuance both kinds share.
///
/// Guardian's `after_encode_and_sign/4` writes a `user_access_tokens` row for
/// `kind in ["user", "app"]` — the same table for both — so an application's
/// token is revoked the same way a user's is, by deleting a row.
async fn issue(
    pool: &PgPool,
    secret: &[u8],
    issuance: Issuance<'_>,
    now: OffsetDateTime,
) -> Result<String, AuthError> {
    let Issuance {
        subject,
        kind,
        scopes,
        ttl,
        name,
    } = issuance;

    let jti = Uuid::new_v4();
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();
    let issued = now.unix_timestamp();

    sqlx::query!(
        "INSERT INTO user_access_tokens (user_id, token_id, expires, name, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $5)",
        subject,
        jti,
        at + ttl,
        name,
        at
    )
    .execute(pool)
    .await?;

    let claims = Claims {
        sub: subject.to_string(),
        exp: issued + ttl.whole_seconds(),
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
/// consent screen's: that one takes `vc.issue` and nothing else, and neither check can be
/// reused for the other.
pub const APP_SCOPES: &[&str] = &["vc.pay", "vc.claim", "vc.contract", "oauth2.register"];

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

    /// And the consent screen's scope is not one of them, which is the reason the two checks
    /// are two: the two sets do not overlap.
    #[test]
    fn an_app_scope_that_is_not_one_is_refused() {
        assert!(!app_scopes_are_valid(&["vc.issue"]));
        assert!(!app_scopes_are_valid(&["vc.pay", "root"]));
    }
}
