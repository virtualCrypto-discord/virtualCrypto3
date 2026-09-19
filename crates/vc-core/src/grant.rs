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
/// second code of the same pair harmless, and what makes writing the same grant
/// twice harmless.
///
/// Takes a connection rather than a pool because a grant is written both on its
/// own — a guild saying yes from Discord — and inside the transaction that
/// answers a request.
pub async fn create_grant_scopes(
    connection: &mut sqlx::PgConnection,
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
    .execute(&mut *connection)
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

    let mut connection = pool
        .acquire()
        .await
        .map_err(|_| ExchangeError::InvalidCode)?;

    create_grant_scopes(&mut connection, grant_id, &taken.scopes, now)
        .await
        .map_err(|_| ExchangeError::InvalidCode)?;

    drop(connection);

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

/// The grant an application holds in a guild, if it holds one.
///
/// A client credentials request names a guild it wants a token for, and a guild
/// the application has never been granted anything in is not one it can have.
pub async fn grant_for(pool: &PgPool, application_id: i64, guild_id: i64) -> Result<Option<i64>> {
    let found = sqlx::query_scalar!(
        "SELECT id FROM grants WHERE application_id = $1 AND guild_id = $2",
        application_id,
        guild_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(found)
}

/// The grant's row and the scopes it carries, written as one act.
///
/// The upsert does not touch `latest_code`: that column is how a code exchange
/// tells a spent code from one that was never issued, and a guild saying yes from
/// Discord is not a code.
async fn write_grant(
    connection: &mut sqlx::PgConnection,
    application_id: i64,
    guild_id: i64,
    scopes: &[String],
    now: OffsetDateTime,
) -> Result<i64> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let grant_id = sqlx::query_scalar!(
        "INSERT INTO grants (application_id, guild_id, inserted_at, updated_at)
         VALUES ($1, $2, $3, $3)
         ON CONFLICT (application_id, guild_id) DO UPDATE SET updated_at = EXCLUDED.updated_at
         RETURNING id",
        application_id,
        guild_id,
        at
    )
    .fetch_one(&mut *connection)
    .await?;

    create_grant_scopes(connection, grant_id, scopes, now).await?;

    Ok(grant_id)
}

/// A guild saying yes without a browser in the way: what the application's own
/// page writes for its owner.
///
/// Not the Elixir's. There, a grant was only ever a side effect of redeeming an
/// authorization code, which is why an application could not be allowed anything
/// without a person opening an authorization URL. Discord's own yes goes through
/// [`decide_request`], because it answers an ask; this one is the owner acting
/// for a guild they administer, so the ask is theirs to skip.
pub async fn allow_in_guild(
    pool: &PgPool,
    application_id: i64,
    guild_id: i64,
    scopes: &[&str],
    now: OffsetDateTime,
) -> Result<i64> {
    let scopes: Vec<String> = scopes.iter().map(|scope| (*scope).to_string()).collect();
    let mut connection = pool.acquire().await?;

    write_grant(&mut connection, application_id, guild_id, &scopes, now).await
}

/// What taking a permission back did: which application, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revoked {
    /// A pending ask's `user_code` un-asked it: the application keeps nothing
    /// to poll for.
    Ask(i64),
    /// A granted application's `client_id` dropped the issuing scope: the
    /// grant row stays and the scope is gone.
    Grant(i64),
}

/// Take a permission back, by the code the guild was shown or the application's
/// own id.
///
/// A pending ask's `user_code` un-asks it: the row stays, decided as nothing,
/// and the application's poll reads the ask as gone. A granted application's
/// `client_id` drops the issuing scope instead, and a guild token already issued
/// stops issuing because its scopes are read from the grant. `None` is neither
/// having been there to take back.
pub async fn revoke_grant(pool: &PgPool, code: &str, guild_id: i64) -> Result<Option<Revoked>> {
    let unasked = sqlx::query!(
        r#"UPDATE grant_requests SET status = 'approved', updated_at = $3
          WHERE guild_id = $1 AND user_code = $2 AND status = 'pending'
        RETURNING application_id"#,
        guild_id,
        code,
        crate::model::utc_now()
    )
    .fetch_optional(pool)
    .await?;

    if let Some(unasked) = unasked {
        return Ok(Some(Revoked::Ask(unasked.application_id)));
    }

    let Ok(client_id) = Uuid::parse_str(code) else {
        return Ok(None);
    };

    let ungranted = sqlx::query!(
        r#"WITH ungranted AS (
             DELETE FROM grant_scopes
              WHERE scope = 'vc.issue'::virtual_crypto_scope_type
                AND grant_id IN (SELECT g.id FROM grants g
                                  JOIN applications a ON a.id = g.application_id
                                 WHERE a.client_id = $1 AND g.guild_id = $2)
            RETURNING grant_id
           )
           SELECT g.application_id FROM grants g
             JOIN applications a ON a.id = g.application_id
            WHERE a.client_id = $1 AND g.guild_id = $2
              AND EXISTS (SELECT 1 FROM ungranted)"#,
        client_id,
        guild_id
    )
    .fetch_optional(pool)
    .await?;

    match ungranted {
        Some(row) => Ok(row.application_id.map(Revoked::Grant)),
        None => Ok(None),
    }
}

/// What a guild granted an application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantedGuild {
    pub guild_id: i64,
    pub scopes: Vec<String>,
    pub updated_at: PrimitiveDateTime,
}

/// Every guild an application holds a grant in, with what it may do there.
///
/// `guild_id <> 0` is not decoration: a code exchange whose authorization request
/// named no guild writes the sentinel `unwrap_or_default` leaves behind, and that
/// row is not a guild anything can be done in.
pub async fn grants_of(pool: &PgPool, application_id: i64) -> Result<Vec<GrantedGuild>> {
    let rows = sqlx::query!(
        r#"SELECT g.guild_id AS "guild_id!",
                  g.updated_at,
                  COALESCE(array_agg(s.scope::text) FILTER (WHERE s.scope IS NOT NULL),
                           ARRAY[]::text[]) AS "scopes!"
             FROM grants g
             LEFT JOIN grant_scopes s ON s.grant_id = g.id
            WHERE g.application_id = $1 AND g.guild_id IS NOT NULL AND g.guild_id <> 0
            GROUP BY g.id, g.guild_id, g.updated_at
            ORDER BY g.guild_id"#,
        application_id
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| GrantedGuild {
            guild_id: row.guild_id,
            scopes: row.scopes,
            updated_at: row.updated_at,
        })
        .collect())
}

/// What a guild token turned out to be: which application, for which guild, and
/// what that grant carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenGrant {
    pub application_id: i64,
    /// The application's own account — `users.id`, which is an `integer` here and
    /// not the `bigint` its Discord ids are — and the identity an idempotent
    /// request is filed under.
    pub account_id: i32,
    pub guild_id: i64,
    pub scopes: Vec<String>,
}

/// A guild token, resolved: the grant an `access_tokens` row belongs to.
///
/// This is the `guild` kind, and it is a row rather than a JWT because that is
/// what the code flow has always handed out — `Authz.md` calls the claim a kind,
/// and the kind here is the token's provenance. `None` is every way a token can
/// fail to be one: unparsable, expired, revoked, or issued for a grant with no
/// guild in it.
pub async fn resolve_token(
    pool: &PgPool,
    token_id: Uuid,
    now: OffsetDateTime,
) -> Result<Option<TokenGrant>> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let found = sqlx::query!(
        r#"SELECT g.application_id AS "application_id!",
                  u.id AS "account_id!",
                  g.guild_id AS "guild_id!",
                  COALESCE(array_agg(s.scope::text) FILTER (WHERE s.scope IS NOT NULL),
                           ARRAY[]::text[]) AS "scopes!"
             FROM access_tokens t
             JOIN grants g ON g.id = t.grant_id
             JOIN users u ON u.application_id = g.application_id
             LEFT JOIN grant_scopes s ON s.grant_id = g.id
            WHERE t.token_id = $1
              AND t.expires >= $2
              AND g.guild_id IS NOT NULL AND g.guild_id <> 0
            GROUP BY g.id, u.id"#,
        token_id,
        at
    )
    .fetch_optional(pool)
    .await?;

    Ok(found.map(|row| TokenGrant {
        application_id: row.application_id,
        account_id: row.account_id,
        guild_id: row.guild_id,
        scopes: row.scopes,
    }))
}

/// An application asking a guild for a permission.
///
/// The ask names the scopes it wants, because the grant it may become is
/// written from exactly those: an approval that granted something else would be
/// a permission nobody asked for. Asking twice is the same ask, so a second
/// request while one is pending keeps the request that is there — the codes
/// with it, so the application's poll and the guild's screen keep agreeing.
///
/// `expires_in` is how long the ask lives, in seconds, and it is the caller's
/// to name within reason: a device that will poll for ten minutes asks for ten
/// minutes, and the guild's screen stops showing it after that.
pub async fn request_grant(
    pool: &PgPool,
    application_id: i64,
    guild_id: i64,
    scopes: &[String],
    expires_in: i64,
    now: OffsetDateTime,
) -> Result<GrantRequest> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let row = sqlx::query!(
        r#"INSERT INTO grant_requests (application_id, guild_id, scopes, device_code, user_code, expires_in, inserted_at, updated_at)
         VALUES ($1, $2, $3, gen_random_uuid(), substring(md5(random()::text) from 1 for 8), $4, $5, $5)
         ON CONFLICT (application_id, guild_id) WHERE status = 'pending'
         DO UPDATE SET updated_at = EXCLUDED.updated_at
         RETURNING id, scopes AS "scopes!: Vec<String>", device_code AS "device_code!: Uuid",
                   user_code AS "user_code!: String", status AS "status!: String", expires_in AS "expires_in!: i64""#,
        application_id,
        guild_id,
        scopes,
        expires_in,
        at
    )
    .fetch_one(pool)
    .await?;

    Ok(GrantRequest {
        id: row.id,
        application_id,
        guild_id,
        scopes: row.scopes,
        device_code: row.device_code,
        user_code: row.user_code,
        status: row.status,
        expires_in: row.expires_in,
    })
}

/// A pending request as the guild's command lists it: the code the administrator
/// types, the name to show, and the scopes being asked for — the three the
/// approval is an answer to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequest {
    pub id: i64,
    pub application_id: i64,
    pub client_id: String,
    pub client_name: Option<String>,
    pub user_code: String,
    pub scopes: Vec<String>,
}

/// What a guild has been asked for and not yet answered, and still alive:
/// an ask older than its `expires_in` is not shown, because the application
/// has already been told it is gone.
pub async fn requests_in_guild(
    pool: &PgPool,
    guild_id: i64,
    now: OffsetDateTime,
) -> Result<Vec<PendingRequest>> {
    let rows = sqlx::query!(
        r#"SELECT r.id, r.application_id, a.client_id::text AS "client_id!", a.client_name,
                  r.user_code AS "user_code!: String", r.scopes AS "scopes!: Vec<String>",
                  EXTRACT(EPOCH FROM (r.inserted_at + make_interval(secs => r.expires_in)))::bigint AS "expires_at!: i64"
             FROM grant_requests r
             JOIN applications a ON a.id = r.application_id
            WHERE r.guild_id = $1 AND r.status = 'pending'
            ORDER BY r.id"#,
        guild_id
    )
    .fetch_all(pool)
    .await?;

    let now = now.unix_timestamp();

    Ok(rows
        .into_iter()
        .filter(|row| row.expires_at > now)
        .map(|row| PendingRequest {
            id: row.id,
            application_id: row.application_id,
            client_id: row.client_id,
            client_name: row.client_name,
            user_code: row.user_code,
            scopes: row.scopes,
        })
        .collect())
}

/// A request as the application that made it reads it: the codes it polls with
/// and shows, the scopes it asked for, and how long the ask lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRequest {
    pub id: i64,
    pub application_id: i64,
    pub guild_id: i64,
    pub scopes: Vec<String>,
    pub device_code: Uuid,
    pub user_code: String,
    pub status: String,
    pub expires_in: i64,
}

/// What an application has asked for, answered or not.
pub async fn requests_of(pool: &PgPool, application_id: i64) -> Result<Vec<GrantRequest>> {
    let rows = sqlx::query!(
        r#"SELECT id, application_id, guild_id, scopes AS "scopes!",
                  device_code AS "device_code!: Uuid", user_code AS "user_code!",
                  status AS "status!", expires_in AS "expires_in!"
             FROM grant_requests
            WHERE application_id = $1
            ORDER BY id"#,
        application_id
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| GrantRequest {
            id: row.id,
            application_id: row.application_id,
            guild_id: row.guild_id,
            scopes: row.scopes,
            device_code: row.device_code,
            user_code: row.user_code,
            status: row.status,
            expires_in: row.expires_in,
        })
        .collect())
}

/// The guild's answer to a request: the status, and for a yes the grant itself.
///
/// The grant is written from the ask's own scopes, never from anything the
/// caller names: an approval that granted something else would be a permission
/// nobody asked for. There is no no — `None` is a request that is not this
/// guild's, not there, expired, or already answered, and all four are answered
/// the same way, because an unapproved ask simply stays pending until it dies.
///
/// One transaction, because a request answered yes that granted nothing is a
/// state the application reads as approval and acts on.
pub async fn decide_request(
    pool: &PgPool,
    user_code: &str,
    guild_id: i64,
    now: OffsetDateTime,
) -> Result<Option<i64>> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let mut tx = pool.begin().await?;

    let decided = sqlx::query!(
        r#"UPDATE grant_requests
              SET status = 'approved', updated_at = $3
            WHERE guild_id = $1 AND user_code = $2 AND status = 'pending'
              AND inserted_at + make_interval(secs => expires_in) > $3
         RETURNING application_id, scopes AS "scopes!""#,
        guild_id,
        user_code,
        at
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(decided) = decided else {
        return Ok(None);
    };

    write_grant(
        &mut tx,
        decided.application_id,
        guild_id,
        &decided.scopes,
        now,
    )
    .await?;

    tx.commit().await?;

    Ok(Some(decided.application_id))
}

/// A device poll: what the application's `device_code` names, while it is still
/// alive.
///
/// `None` is every way a poll can fail to name an ask: unknown, decided, or
/// expired. A device must not learn which of those it was — an `invalid_grant`
/// is the whole of the answer either way — so the three are one `None` here
/// and the caller answers them as one.
pub async fn poll_request(
    pool: &PgPool,
    application_id: i64,
    device_code: Uuid,
    now: OffsetDateTime,
) -> Result<Option<GrantRequest>> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let row = sqlx::query!(
        r#"SELECT id, application_id, guild_id, scopes AS "scopes!",
                  device_code AS "device_code!: Uuid", user_code AS "user_code!",
                  status AS "status!", expires_in AS "expires_in!"
             FROM grant_requests
            WHERE application_id = $1 AND device_code = $2
              AND inserted_at + make_interval(secs => expires_in) > $3"#,
        application_id,
        device_code,
        at
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| GrantRequest {
        id: row.id,
        application_id: row.application_id,
        guild_id: row.guild_id,
        scopes: row.scopes,
        device_code: row.device_code,
        user_code: row.user_code,
        status: row.status,
        expires_in: row.expires_in,
    }))
}

/// `revoke_access_token/1`: forget an access token.
///
/// `true` when a row went, which is the difference between revoking a token and
/// being told about one that was never there. RFC 7009 has the endpoint answer
/// `200` either way, so a client may ignore it; a test may not.
///
/// This is one of the two functions the Elixir has and its controller never
/// calls — the revocation endpoint only knew how to revoke a JWT. See
/// docs/oauth2.md.
pub async fn revoke_access_token(pool: &PgPool, token: &str) -> Result<bool> {
    let Ok(token_id) = Uuid::parse_str(token) else {
        return Ok(false);
    };

    let deleted = sqlx::query!("DELETE FROM access_tokens WHERE token_id = $1", token_id)
        .execute(pool)
        .await?;

    Ok(deleted.rows_affected() > 0)
}

/// `revoke_refresh_token/1`: forget a refresh token, which ends the ability to
/// refresh but leaves the access tokens already issued from that grant alone.
pub async fn revoke_refresh_token(pool: &PgPool, token: &str) -> Result<bool> {
    let Ok(token_id) = Uuid::parse_str(token) else {
        return Ok(false);
    };

    let deleted = sqlx::query!("DELETE FROM refresh_tokens WHERE token_id = $1", token_id)
        .execute(pool)
        .await?;

    Ok(deleted.rows_affected() > 0)
}
