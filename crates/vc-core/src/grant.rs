//! OAuth2 grants, and the tokens that belong to them.

use sqlx::PgPool;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use crate::application::{find_by_client_id, redirect_uri_is_registered, take_code};
use crate::error::Result;

/// How long an access token is good for: the Elixir's `3600` seconds.
pub const ACCESS_TOKEN_TTL: Duration = Duration::hours(1);

/// `AccessToken.create_access_token/2`: a token for a grant.
///
/// The token a client receives *is* this row's `token_id`, which is why revoking
/// one is a delete rather than waiting for something to expire. It is not a JWT,
/// unlike the tokens the REST API hands out, and it is not signed by anything.
///
/// The Elixir carries a retry here — five attempts, "if the insert comes back
/// without an id" — and it cannot run: it looks the row up with
/// `Repo.get/2` and a keyword list, and `Repo.get_by/2` is the function that
/// accepts one, so the single path that reaches it raises. An insert that fails
/// is not retried by wrapping it in a branch that also fails, so there is no
/// retry here.
pub async fn create_access_token(
    pool: &PgPool,
    grant_id: i64,
    now: OffsetDateTime,
) -> Result<String> {
    let token_id = Uuid::new_v4();
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    sqlx::query!(
        "INSERT INTO access_tokens (grant_id, token_id, expires, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)",
        grant_id,
        token_id,
        at + ACCESS_TOKEN_TTL,
        at
    )
    .execute(pool)
    .await?;

    Ok(token_id.to_string())
}

/// `Grant.get_or_create_grant_if_not_reused/3`: the grant a code belongs to.
///
/// A code that a grant still remembers has already been redeemed, and that is
/// what the delete in front is for: it is not cleaning up, it is the check. Two
/// codes for the same application and guild are the same grant, so the insert
/// conflicts on that pair and keeps the newer code.
///
/// `None` is that reuse, and the caller answers it as `used_code`.
pub async fn grant_for_code(
    pool: &PgPool,
    application_id: i64,
    guild_id: i64,
    latest_code: &str,
    now: OffsetDateTime,
) -> Result<Option<i64>> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let remembered = sqlx::query!("DELETE FROM grants WHERE latest_code = $1", latest_code)
        .execute(pool)
        .await?;

    if remembered.rows_affected() > 0 {
        return Ok(None);
    }

    let id = sqlx::query_scalar!(
        "INSERT INTO grants (application_id, guild_id, latest_code, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)
         ON CONFLICT (application_id, guild_id) DO UPDATE SET latest_code = $3
        RETURNING id",
        application_id,
        guild_id,
        latest_code,
        at
    )
    .fetch_one(pool)
    .await?;

    Ok(Some(id))
}

/// `Grant.create_grant_scopes/2`: what a grant carries.
///
/// One statement for however many scopes, as `insert_all/3` is, rather than one
/// per scope — the same care `UserResolver.resolve_ids/1` takes and for the same
/// reason. Conflicting rows are left alone, which is what makes redeeming a
/// second code of the same pair harmless.
pub async fn create_grant_scopes(
    pool: &PgPool,
    grant_id: i64,
    scopes: &[String],
    now: OffsetDateTime,
) -> Result<()> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    sqlx::query!(
        "INSERT INTO grant_scopes (grant_id, scope, inserted_at, updated_at)
         SELECT $1, t.scope::text::virtual_crypto_scope_type, $3, $3
           FROM UNNEST($2::text[]) AS t(scope)
         ON CONFLICT DO NOTHING",
        grant_id,
        scopes,
        at
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// How long a refresh token lasts: the Elixir's `180 * 24 * 60 * 60` seconds.
pub const REFRESH_TOKEN_TTL: Duration = Duration::days(180);

/// `RefreshToken.create_refresh_token/2`: the grant's refresh token, replacing
/// whatever it had.
///
/// One per grant — the insert conflicts on `grant_id` — which is what makes a
/// refresh token a thing that can be rotated out of existence rather than
/// accumulated.
///
/// The Elixir's five-attempt retry is not here for the same reason it is absent
/// from `create_access_token`: it looks the row up with `Repo.get/2` and a
/// keyword list, so the one path that reaches it raises. The same unreachable
/// branch appears in both functions, which is what a copy is.
pub async fn create_refresh_token(
    pool: &PgPool,
    grant_id: i64,
    now: OffsetDateTime,
) -> Result<String> {
    let token_id = Uuid::new_v4();
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    // No `RETURNING`: the column is nullable, so it would come back as an
    // `Option`, and the value to answer with is the one just written.
    sqlx::query!(
        "INSERT INTO refresh_tokens (grant_id, token_id, expires, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)
         ON CONFLICT (grant_id) DO UPDATE
            SET token_id = $2, expires = $3, updated_at = $4",
        grant_id,
        token_id,
        at + REFRESH_TOKEN_TTL,
        at
    )
    .execute(pool)
    .await?;

    Ok(token_id.to_string())
}

/// `RefreshToken.replace_refresh_token/2`: swap a live token for a new one,
/// answering with the grant it belonged to.
///
/// The expiry is part of the match, so an expired token cannot be rotated: it is
/// not found rather than given another six months. `None` is that, and the
/// caller answers it as `invalid_refresh_token`.
pub async fn replace_refresh_token(
    pool: &PgPool,
    old_token_id: Uuid,
    now: OffsetDateTime,
) -> Result<Option<(i64, String)>> {
    let new_token_id = Uuid::new_v4();
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let replaced = sqlx::query!(
        "UPDATE refresh_tokens
            SET token_id = $1, updated_at = $3
          WHERE token_id = $2 AND expires >= $3
        RETURNING grant_id",
        new_token_id,
        old_token_id,
        at
    )
    .fetch_optional(pool)
    .await?;

    Ok(replaced
        .and_then(|row| row.grant_id)
        .map(|grant_id| (grant_id, new_token_id.to_string())))
}

/// An hour, as the literal the Elixir writes for the code exchange. The refresh
/// exchange computes the same hour from the clock, and the difference is
/// reproduced rather than harmonised.
pub const EXPIRES_IN: i64 = 3600;

/// Why a code could not be exchanged, in the API's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeError {
    /// `invalid_grant`, `invalid_code`: never issued, or past its fifteen
    /// minutes. The Elixir answers both the same way, so this does too.
    InvalidCode,
    /// `invalid_grant`, `used_code`: spent. A client can act on the difference.
    UsedCode,
    /// `invalid_request`, `not_found_client`.
    NotFoundClient,
    /// `invalid_grant`, `issued_to_other_client`.
    IssuedToOtherClient,
    /// `invalid_grant`, `redirect_uri_mismatch`.
    RedirectUriMismatch,
    /// `invalid_grant`, `invalid_refresh_token`: unknown, expired, or already
    /// rotated away by the exchange that replaced it.
    InvalidRefreshToken,
}

impl ExchangeError {
    pub fn error(self) -> &'static str {
        match self {
            ExchangeError::InvalidCode | ExchangeError::UsedCode => "invalid_grant",
            ExchangeError::NotFoundClient => "invalid_request",
            ExchangeError::IssuedToOtherClient
            | ExchangeError::RedirectUriMismatch
            | ExchangeError::InvalidRefreshToken => "invalid_grant",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ExchangeError::InvalidCode => "invalid_code",
            ExchangeError::UsedCode => "used_code",
            ExchangeError::NotFoundClient => "not_found_client",
            ExchangeError::IssuedToOtherClient => "issued_to_other_client",
            ExchangeError::RedirectUriMismatch => "redirect_uri_mismatch",
            ExchangeError::InvalidRefreshToken => "invalid_refresh_token",
        }
    }
}

/// What a code is exchanged for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchanged {
    pub access_token: String,
    /// Present only if the application lists `refresh_token` among its grant
    /// types, which is how an application opts out of being refreshable.
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
    pub expires_in: i64,
}

/// Whether some grant still remembers this code.
///
/// The other half of telling a spent code from one that was never issued: the
/// grant keeps the code it was last redeemed with, so a code in a grant's memory
/// was spent, and one nowhere at all is a code that never existed.
async fn grant_remembers_code(pool: &PgPool, code: &str) -> std::result::Result<bool, sqlx::Error> {
    let found = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM grants WHERE latest_code = $1) AS "exists!""#,
        code
    )
    .fetch_one(pool)
    .await?;

    Ok(found)
}

/// `token_authorization_code/4`.
///
/// The order is the Elixir's and it is the contract: the code is spent first
/// (taking it is what spends it), then its expiry, then the application, then
/// that the code was issued to *this* application, then the redirect URI, and
/// only then a grant and its tokens.
pub async fn exchange_code(
    pool: &PgPool,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    now: OffsetDateTime,
) -> std::result::Result<Exchanged, ExchangeError> {
    let taken = match take_code(pool, code).await {
        Ok(Some(taken)) => taken,
        Ok(None) => {
            return Err(
                match grant_remembers_code(pool, code).await.unwrap_or(false) {
                    true => ExchangeError::UsedCode,
                    false => ExchangeError::InvalidCode,
                },
            );
        }
        Err(_) => return Err(ExchangeError::InvalidCode),
    };

    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    if taken.expires.is_none_or(|expires| expires <= at) {
        return Err(ExchangeError::InvalidCode);
    }

    let application = find_by_client_id(pool, client_id)
        .await
        .map_err(|_| ExchangeError::NotFoundClient)?
        .ok_or(ExchangeError::NotFoundClient)?;

    if taken.application_id != Some(application.id) {
        return Err(ExchangeError::IssuedToOtherClient);
    }

    if !redirect_uri_is_registered(pool, application.id, redirect_uri)
        .await
        .unwrap_or(false)
    {
        return Err(ExchangeError::RedirectUriMismatch);
    }

    let granted = grant_for_code(
        pool,
        application.id,
        taken.guild_id.unwrap_or_default(),
        code,
        now,
    )
    .await
    .map_err(|_| ExchangeError::InvalidCode)?;

    let Some(grant_id) = granted else {
        return Err(ExchangeError::UsedCode);
    };

    create_grant_scopes(pool, grant_id, &taken.scopes, now)
        .await
        .map_err(|_| ExchangeError::InvalidCode)?;

    let access_token = create_access_token(pool, grant_id, now)
        .await
        .map_err(|_| ExchangeError::InvalidCode)?;

    let refresh_token = if application
        .grant_types
        .iter()
        .any(|grant| grant == "refresh_token")
    {
        Some(
            create_refresh_token(pool, grant_id, now)
                .await
                .map_err(|_| ExchangeError::InvalidCode)?,
        )
    } else {
        None
    };

    Ok(Exchanged {
        access_token,
        refresh_token,
        scopes: taken.scopes,
        expires_in: EXPIRES_IN,
    })
}

/// What a refresh token is exchanged for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub access_token: String,
    /// The replacement. There is always one: refreshing retires the token that
    /// was presented, which is what stops a stolen one being useful twice.
    pub refresh_token: String,
}

/// `token_refresh_token/1`: a new access token, and a new refresh token with it.
///
/// Its answer carries no `scopes`, unlike the code exchange's. The grant knows
/// them and the client is expected to have kept them; the two exchanges
/// disagreeing about that is the Elixir's, and is reproduced.
pub async fn exchange_refresh_token(
    pool: &PgPool,
    refresh_token: &str,
    now: OffsetDateTime,
) -> std::result::Result<Refreshed, ExchangeError> {
    // The token is a row's `token_id`, so anything that is not a UUID is not a
    // token rather than a database error.
    let Ok(presented) = Uuid::parse_str(refresh_token) else {
        return Err(ExchangeError::InvalidRefreshToken);
    };

    let replaced = replace_refresh_token(pool, presented, now)
        .await
        .map_err(|_| ExchangeError::InvalidRefreshToken)?
        .ok_or(ExchangeError::InvalidRefreshToken)?;

    let (grant_id, refresh_token) = replaced;

    let access_token = create_access_token(pool, grant_id, now)
        .await
        .map_err(|_| ExchangeError::InvalidRefreshToken)?;

    Ok(Refreshed {
        access_token,
        refresh_token,
    })
}
