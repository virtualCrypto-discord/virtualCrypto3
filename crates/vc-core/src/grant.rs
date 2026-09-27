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
pub async fn create_access_token<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    executor: E,
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
    .execute(executor)
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
    let mut connection = pool.acquire().await?;
    grant_for_code_in(
        &mut connection,
        application_id,
        guild_id,
        latest_code,
        now,
        false,
    )
    .await
}

async fn grant_for_code_in(
    connection: &mut sqlx::PgConnection,
    application_id: i64,
    guild_id: i64,
    latest_code: &str,
    now: OffsetDateTime,
    independent: bool,
) -> Result<Option<i64>> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let remembered = sqlx::query!("DELETE FROM grants WHERE latest_code = $1", latest_code)
        .execute(&mut *connection)
        .await?;

    if remembered.rows_affected() > 0 {
        return Ok(None);
    }

    let id = sqlx::query_scalar!(
        "INSERT INTO grants (application_id, guild_id, latest_code, inserted_at, updated_at, independent)
         VALUES ($1, $2, $3, $4, $4, $5)
         ON CONFLICT (application_id, guild_id) WHERE NOT independent DO UPDATE SET latest_code = $3
        RETURNING id",
        application_id,
        guild_id,
        latest_code,
        at,
        independent
    )
    .fetch_one(&mut *connection)
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

/// What a grant is *for*: the currencies it may be used in, as `grant_resources`
/// rows. The array scopes beside it is what it may *do*.
///
/// An empty set is every currency of the target, which is what the collection
/// form of RFC 8707's `resource` asks for and what an ask that names nothing
/// asks for too — they are the same fact written two ways, and neither writes a
/// row. It is also what a grant written before resources existed means, so a
/// migration does not have to rewrite those rows.
/// Approved currency ids remain here after currency deletion: removing the last
/// resource would turn a restricted grant into an unrestricted one. A subsequent
/// approval can still replace the set explicitly.
///
/// Replaces the resources on the supplied grant. Independent approvals get a
/// fresh grant id, so this cannot alter an earlier approval. Only legacy code
/// exchanges may reuse a grant and replace its resources for compatibility.
pub async fn create_grant_resources(
    connection: &mut sqlx::PgConnection,
    grant_id: i64,
    resources: &[i64],
    now: OffsetDateTime,
) -> Result<()> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    sqlx::query!("DELETE FROM grant_resources WHERE grant_id = $1", grant_id)
        .execute(&mut *connection)
        .await?;

    sqlx::query!(
        "INSERT INTO grant_resources (grant_id, currency_id, inserted_at, updated_at)
         SELECT $1, t.currency_id, $3, $3
           FROM UNNEST($2::bigint[]) AS t(currency_id)",
        grant_id,
        resources,
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
pub async fn create_refresh_token<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    executor: E,
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
    .execute(executor)
    .await?;

    Ok(token_id.to_string())
}

/// `RefreshToken.replace_refresh_token/2`: swap a live token for a new one,
/// answering with the grant it belonged to.
///
/// The expiry is part of the match, so an expired token cannot be rotated: it is
/// not found rather than given another six months. `None` is that, and the
/// caller answers it as `invalid_refresh_token`.
pub async fn replace_refresh_token<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    executor: E,
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
    .fetch_optional(executor)
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

/// What a code or an approved device request is exchanged for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchanged {
    pub access_token: String,
    /// Present only if the application lists `refresh_token` among its grant
    /// types, which is how an application opts out of being refreshable.
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
    pub expires_in: i64,
}

/// Exchange a code atomically. Ordinary refusals roll back consumption of the
/// code; reuse commits revocation of the grant and all its tokens, as Elixir's
/// `Auth.run/1` does for `{:commit, {:error, ...}}`.
pub async fn exchange_code(
    pool: &PgPool,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    now: OffsetDateTime,
) -> std::result::Result<Exchanged, ExchangeError> {
    let mut tx = pool
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
        .await
        .map_err(|_| ExchangeError::InvalidCode)?;
    let result = exchange_code_in(&mut tx, client_id, redirect_uri, code, now).await;
    if result.is_ok() || result == Err(ExchangeError::UsedCode) {
        tx.commit().await.map_err(|_| ExchangeError::InvalidCode)?;
    } else {
        tx.rollback()
            .await
            .map_err(|_| ExchangeError::InvalidCode)?;
    }
    result
}

async fn exchange_code_in(
    connection: &mut sqlx::PgConnection,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    now: OffsetDateTime,
) -> std::result::Result<Exchanged, ExchangeError> {
    let taken = match take_code(&mut *connection, code).await {
        Ok(Some(taken)) => taken,
        Ok(None) => {
            let revoked = sqlx::query!("DELETE FROM grants WHERE latest_code = $1", code)
                .execute(&mut *connection)
                .await
                .map_err(|_| ExchangeError::InvalidCode)?;
            return Err(if revoked.rows_affected() > 0 {
                ExchangeError::UsedCode
            } else {
                ExchangeError::InvalidCode
            });
        }
        Err(_) => return Err(ExchangeError::InvalidCode),
    };

    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    if taken.expires.is_none_or(|expires| expires <= at) {
        return Err(ExchangeError::InvalidCode);
    }

    let application = find_by_client_id(&mut *connection, client_id)
        .await
        .map_err(|_| ExchangeError::NotFoundClient)?
        .ok_or(ExchangeError::NotFoundClient)?;

    if taken.application_id != Some(application.id) {
        return Err(ExchangeError::IssuedToOtherClient);
    }

    // Registration alone is insufficient: the code belongs to the exact URI
    // used for consent. A mismatch rolls back consumption of the code.
    if taken.redirect_uri.as_deref() != Some(redirect_uri)
        || !redirect_uri_is_registered(&mut *connection, application.id, redirect_uri)
            .await
            .unwrap_or(false)
    {
        return Err(ExchangeError::RedirectUriMismatch);
    }

    let granted = grant_for_code_in(
        &mut *connection,
        application.id,
        taken.guild_id.unwrap_or_default(),
        code,
        now,
        taken
            .scopes
            .iter()
            .any(|scope| scope == crate::application::ISSUE)
            || !taken.resources.is_empty(),
    )
    .await
    .map_err(|_| ExchangeError::InvalidCode)?;

    let Some(grant_id) = granted else {
        return Err(ExchangeError::UsedCode);
    };

    create_grant_scopes(&mut *connection, grant_id, &taken.scopes, now)
        .await
        .map_err(|_| ExchangeError::InvalidCode)?;

    // v3 issuing/resource consent gets its own grant just like device consent.
    // Only legacy scope-less/OIDC exchanges retain the original shared slot.
    create_grant_resources(&mut *connection, grant_id, &taken.resources, now)
        .await
        .map_err(|_| ExchangeError::InvalidCode)?;

    let access_token = create_access_token(&mut *connection, grant_id, now)
        .await
        .map_err(|_| ExchangeError::InvalidCode)?;

    let refresh_token = if application
        .grant_types
        .iter()
        .any(|grant| grant == "refresh_token")
    {
        Some(
            create_refresh_token(&mut *connection, grant_id, now)
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

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ExchangeError::InvalidRefreshToken)?;
    // Revocation locks the grant before cascading to its tokens. Take the same
    // order, locking only the grant here; rotation rechecks the presented token
    // and its expiry after any lock wait.
    sqlx::query_scalar!(
        "SELECT g.id FROM grants g JOIN refresh_tokens r ON r.grant_id = g.id
          WHERE r.token_id = $1 FOR KEY SHARE OF g",
        presented
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ExchangeError::InvalidRefreshToken)?
    .ok_or(ExchangeError::InvalidRefreshToken)?;

    let replaced = replace_refresh_token(&mut *tx, presented, now)
        .await
        .map_err(|_| ExchangeError::InvalidRefreshToken)?
        .ok_or(ExchangeError::InvalidRefreshToken)?;

    let (grant_id, refresh_token) = replaced;

    let access_token = create_access_token(&mut *tx, grant_id, now)
        .await
        .map_err(|_| ExchangeError::InvalidRefreshToken)?;

    tx.commit()
        .await
        .map_err(|_| ExchangeError::InvalidRefreshToken)?;

    Ok(Refreshed {
        access_token,
        refresh_token,
    })
}

/// Who an ask is put to, and who a grant was written for: a guild whose pool the
/// application may issue from, or a person whose account it may act as.
///
/// One value rather than a `guild_id` threaded through, because everything the
/// device flow does — asking, deciding, polling, resolving a token — is the same
/// act with a different answerer, and what differs in the details is the column
/// the id goes in. A row sets exactly one of the two, which is the database's own
/// rule and not a convention this type takes on trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Guild(i64),
    User(i64),
}

impl Target {
    /// The guild the ask names, when it names one.
    pub fn guild(self) -> Option<i64> {
        match self {
            Target::Guild(id) => Some(id),
            Target::User(_) => None,
        }
    }

    /// The person the ask names, when it names one: a Discord id, the way a
    /// contract's `receiver_discord_id` is and a `users.id` is not.
    pub fn user(self) -> Option<i64> {
        match self {
            Target::User(id) => Some(id),
            Target::Guild(_) => None,
        }
    }

    /// What the two columns of a row amount to, when they amount to either.
    fn of(guild_id: Option<i64>, discord_id: Option<i64>) -> Option<Target> {
        match (guild_id, discord_id) {
            (Some(guild), None) => Some(Target::Guild(guild)),
            (None, Some(user)) => Some(Target::User(user)),
            _ => None,
        }
    }
}

/// The legacy grant slot for an application and guild. Device grants are
/// deliberately excluded: they must be addressed by their approval's grant id.
pub async fn grant_for(pool: &PgPool, application_id: i64, guild_id: i64) -> Result<Option<i64>> {
    let found = sqlx::query_scalar!(
        "SELECT id FROM grants WHERE application_id = $1 AND guild_id = $2 AND NOT independent",
        application_id,
        guild_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(found)
}

/// Mint device tokens only while every approved scope is still granted.
/// Lock the scope rows until the token is committed so revocation cannot slip
/// between checking permission and issuing the tokens. Refresh is opt-in, just
/// as it is for the code exchange, and both tokens belong to this exact grant.
/// Exclusively lock the request and mark it exchanged in the same transaction.
pub async fn create_device_token(
    pool: &PgPool,
    request: &GrantRequest,
    now: OffsetDateTime,
) -> Result<Option<Exchanged>> {
    let mut tx = pool.begin().await?;
    let grant = sqlx::query!(
        "SELECT g.id, COALESCE(a.grant_types::text[], ARRAY[]::text[]) AS \"grant_types!\"
           FROM grants g JOIN grant_requests r ON r.grant_id = g.id
           JOIN applications a ON a.id = g.application_id
          WHERE r.id = $1 AND r.application_id = $2 AND r.status = 'approved'
            AND r.inserted_at + make_interval(secs => r.expires_in) > $3
            AND g.guild_id IS NOT DISTINCT FROM $4
            AND g.discord_id IS NOT DISTINCT FROM $5 FOR SHARE OF g FOR UPDATE OF r",
        request.id,
        request.application_id,
        PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second(),
        request.target.guild(),
        request.target.user()
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(grant) = grant else {
        return Ok(None);
    };
    let grant_id = grant.id;
    let scopes = sqlx::query_scalar!(
        r#"SELECT scope::text AS "scope!" FROM grant_scopes
            WHERE grant_id = $1 FOR SHARE"#,
        grant_id
    )
    .fetch_all(&mut *tx)
    .await?;
    if request.scopes.iter().any(|scope| !scopes.contains(scope)) {
        return Ok(None);
    }
    // Check the deadline again after any row-lock wait. Failure to issue either
    // token rolls this transition back, allowing the client to retry.
    let consumed = sqlx::query(
        "UPDATE grant_requests SET status = 'exchanged', updated_at = clock_timestamp() AT TIME ZONE 'utc'
         WHERE id = $1 AND status = 'approved'
           AND inserted_at + make_interval(secs => expires_in) > clock_timestamp() AT TIME ZONE 'utc'",
    )
    .bind(request.id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if consumed == 0 {
        return Ok(None);
    }
    let access_token = create_access_token(&mut *tx, grant_id, now).await?;
    let refresh_token = if grant.grant_types.iter().any(|kind| kind == "refresh_token") {
        Some(create_refresh_token(&mut *tx, grant_id, now).await?)
    } else {
        None
    };
    tx.commit().await?;
    Ok(Some(Exchanged {
        access_token,
        refresh_token,
        scopes,
        expires_in: EXPIRES_IN,
    }))
}

/// The grant's row and the scopes it carries, written as one act.
///
/// The upsert does not touch `latest_code`: that column is how a code exchange
/// tells a spent code from one that was never issued, and a guild saying yes from
/// Discord is not a code.
async fn write_grant(
    connection: &mut sqlx::PgConnection,
    application_id: i64,
    target: Target,
    scopes: &[String],
    resources: &[i64],
    now: OffsetDateTime,
    independent: bool,
) -> Result<i64> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    if let Target::User(discord_id) = target {
        crate::user::insert_if_not_exists(connection, discord_id).await?;
    }

    // Only legacy rows participate in the unique slot. Device approvals always
    // insert a new independent row; another consent cannot alter its authority.
    let grant_id = sqlx::query_scalar!(
        "INSERT INTO grants (application_id, guild_id, discord_id, inserted_at, updated_at, independent)
         VALUES ($1, $2, $3, $4, $4, $5)
         ON CONFLICT (application_id, (COALESCE(guild_id, discord_id))) WHERE NOT independent
         DO UPDATE SET updated_at = EXCLUDED.updated_at
         RETURNING id",
        application_id,
        target.guild(),
        target.user(),
        at,
        independent
    )
    .fetch_one(&mut *connection)
    .await?;

    create_grant_scopes(connection, grant_id, scopes, now).await?;
    create_grant_resources(connection, grant_id, resources, now).await?;

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
///
/// `resources` is the currency ids the permission is narrowed to, and an empty
/// slice is every currency of the guild — the caller has no ask to read them
/// from, so it names them itself.
pub async fn allow_in_guild(
    pool: &PgPool,
    application_id: i64,
    guild_id: i64,
    scopes: &[&str],
    resources: &[i64],
    now: OffsetDateTime,
) -> Result<i64> {
    let scopes: Vec<String> = scopes.iter().map(|scope| (*scope).to_string()).collect();
    let mut connection = pool.acquire().await?;

    write_grant(
        &mut connection,
        application_id,
        Target::Guild(guild_id),
        &scopes,
        resources,
        now,
        false,
    )
    .await
}

/// Take a permission back, by the application's own id.
///
/// The permission is the `vc.issue` scope on the grant the application holds in
/// this guild, so the grant is named by its `client_id` — the thing the guild's
/// screen shows and the thing the application's own page sends. The grant row
/// stays and the scope is gone, and a guild token already issued stops issuing
/// because its scopes are read from the grant. `None` is nothing having been
/// there to take back, which is the answer its caller wanted anyway.
///
/// An ask that is still pending is not this: it is the application's own
/// business until the guild answers it, and a permission nobody was granted
/// cannot be taken away.
pub async fn revoke_grant(pool: &PgPool, code: &str, guild_id: i64) -> Result<Option<i64>> {
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
              AND EXISTS (SELECT 1 FROM ungranted) LIMIT 1"#,
        client_id,
        guild_id
    )
    .fetch_optional(pool)
    .await?;

    match ungranted {
        Some(row) => Ok(row.application_id),
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
                  max(g.updated_at) AS "updated_at!",
                  COALESCE(array_agg(DISTINCT s.scope::text) FILTER (WHERE s.scope IS NOT NULL),
                           ARRAY[]::text[]) AS "scopes!"
             FROM grants g
             LEFT JOIN grant_scopes s ON s.grant_id = g.id
            WHERE g.application_id = $1 AND g.guild_id IS NOT NULL AND g.guild_id <> 0
            GROUP BY g.guild_id
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

/// An application a guild has allowed to issue: the row the guild's screen shows
/// and the subject of the button that takes the permission back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedApplication {
    pub grant_id: i64,
    pub client_id: String,
    pub client_name: Option<String>,
    pub bot_discord_id: Option<i64>,
    /// The currencies the grant covers, as `grant_resources` ids. Empty is every
    /// currency of the guild, which is what the screen names 「すべての通貨」, so
    /// that a narrowed grant and one that named nothing can be told apart.
    pub resources: Vec<i64>,
}

/// A page of them, and where the pages around it are — the shape a screen that
/// shows five rows and four arrows needs, as `OpenContracts` is for contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedApplications {
    pub applications: Vec<AuthorizedApplication>,
    /// How many there are altogether, which is the number the screen may say.
    pub total: i64,
    /// Which page this is, counting from one, as the claim list's does.
    pub page: i64,
    /// The four places the arrows move to, each `None` when there is nowhere to
    /// go — which is what makes a button disabled rather than absent.
    pub first: Option<i64>,
    pub prev: Option<i64>,
    pub next: Option<i64>,
    pub last: Option<i64>,
}

/// One page of the applications this guild has allowed to issue, newest grant
/// first, and the count behind it.
///
/// The filter is in the statement rather than in the caller: only a grant that
/// carries `vc.issue` is one the guild can take anything back from, and a page
/// filled with grants that carry something else would hide the ones it can.
///
/// A pending ask is not here at all. It is the application's own business until
/// the guild answers it, and the application holds both codes it needs to hear
/// the answer with.
pub async fn authorized_in_guild(
    pool: &PgPool,
    guild_id: i64,
    page: i64,
    limit: i64,
) -> Result<AuthorizedApplications> {
    let page = page.max(1);

    let total = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM grants g
            WHERE g.guild_id = $1
              AND EXISTS (SELECT 1 FROM grant_scopes s
                           WHERE s.grant_id = g.id
                             AND s.scope = 'vc.issue'::virtual_crypto_scope_type)"#,
        guild_id
    )
    .fetch_one(pool)
    .await?;

    let rows = sqlx::query!(
        r#"SELECT g.id AS grant_id, a.client_id::text AS "client_id!", a.client_name,
                  bot.discord_id AS bot_discord_id,
                  COALESCE((SELECT array_agg(r.currency_id) FROM grant_resources r
                             WHERE r.grant_id = g.id), ARRAY[]::bigint[]) AS "resources!"
             FROM grants g
             JOIN applications a ON a.id = g.application_id
             LEFT JOIN users bot ON bot.application_id = a.id
            WHERE g.guild_id = $1
              AND EXISTS (SELECT 1 FROM grant_scopes s
                           WHERE s.grant_id = g.id
                             AND s.scope = 'vc.issue'::virtual_crypto_scope_type)
            ORDER BY g.inserted_at DESC, g.id DESC
            LIMIT $2
           OFFSET $3"#,
        guild_id,
        limit,
        limit * (page - 1)
    )
    .fetch_all(pool)
    .await?;

    let last_page = ((total + limit - 1) / limit).max(1);

    Ok(AuthorizedApplications {
        applications: rows
            .into_iter()
            .map(|row| AuthorizedApplication {
                grant_id: row.grant_id,
                client_id: row.client_id,
                client_name: row.client_name,
                bot_discord_id: row.bot_discord_id,
                resources: row.resources,
            })
            .collect(),
        total,
        page,
        first: (page > 1).then_some(1),
        prev: (page > 1).then_some(page - 1),
        next: (page < last_page).then_some(page + 1),
        last: (page < last_page).then_some(last_page),
    })
}

/// What a grant token turned out to be: which application, for which target,
/// what that grant carries, and which currencies it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenGrant {
    /// Stable across access-token refreshes, distinct for independent approvals.
    pub grant_id: i64,
    pub application_id: i64,
    /// Whose account the token acts as. For a guild grant it is the application's
    /// own — `users.id`, which is an `integer` here and not the `bigint` its
    /// Discord ids are, and the identity an idempotent request is filed under —
    /// and for a personal grant it is the account of the person who approved it.
    pub account_id: i32,
    pub target: Target,
    pub scopes: Vec<String>,
    /// The currencies the token is worth, as `grant_resources` rows. Empty is
    /// every currency of the target, which is what a grant written before this
    /// existed carries — see [`create_grant_resources`].
    pub resources: Vec<i64>,
}

/// A grant token, resolved: the grant an `access_tokens` row belongs to.
///
/// This is the `guild` kind of `Authz.md`, and it is a row rather than a JWT
/// because that is what the code flow has always handed out — the kind is the
/// token's provenance, not a claim. A personal grant's token is the same row for
/// the same reason, and what tells the two apart is the target: the account a
/// resolution names is the application's own for a guild grant and the approving
/// user's for a personal one.
///
/// `None` is every way a token can fail to be one: unparsable, expired, revoked,
/// issued for a grant with no target in it, or naming a target whose account does
/// not exist.
pub async fn resolve_token(
    pool: &PgPool,
    token_id: Uuid,
    now: OffsetDateTime,
) -> Result<Option<TokenGrant>> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let found = sqlx::query!(
        r#"SELECT g.id AS grant_id, g.application_id AS "application_id!",
                  CASE WHEN g.guild_id IS NOT NULL AND g.guild_id <> 0
                       THEN a.id ELSE p.id END AS "account_id!",
                  g.guild_id,
                  g.discord_id,
                  COALESCE(array_agg(s.scope::text) FILTER (WHERE s.scope IS NOT NULL),
                           ARRAY[]::text[]) AS "scopes!",
                  COALESCE((SELECT array_agg(r.currency_id) FROM grant_resources r
                             WHERE r.grant_id = g.id), ARRAY[]::bigint[]) AS "resources!"
             FROM access_tokens t
             JOIN grants g ON g.id = t.grant_id
             LEFT JOIN users a ON a.application_id = g.application_id
             LEFT JOIN users p ON p.discord_id = g.discord_id
             LEFT JOIN grant_scopes s ON s.grant_id = g.id
            WHERE t.token_id = $1
              AND t.expires >= $2
              AND ((g.guild_id IS NOT NULL AND g.guild_id <> 0) OR g.discord_id IS NOT NULL)
              AND (a.id IS NOT NULL OR p.id IS NOT NULL)
            GROUP BY g.id, a.id, p.id"#,
        token_id,
        at
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = found else {
        return Ok(None);
    };

    let Some(target) = Target::of(row.guild_id, row.discord_id) else {
        return Ok(None);
    };

    Ok(Some(TokenGrant {
        grant_id: row.grant_id,
        application_id: row.application_id,
        account_id: row.account_id,
        target,
        scopes: row.scopes,
        resources: row.resources,
    }))
}

/// An application asking for a permission: a guild's pool, or a person's account.
///
/// The ask names the scopes it wants, because the grant it may become is
/// written from exactly those: an approval that granted something else would be
/// a permission nobody asked for. Retrying an identical pending scope/resource
/// set keeps its codes and deadline. Different permission sets may coexist.
///
/// `expires_in` is how long the ask lives, in seconds, and it is the caller's
/// to name within reason: a device that will poll for ten minutes asks for ten
/// minutes, and the screen stops showing it after that.
///
/// `resources` is what the ask is *for*: the currency ids RFC 8707's `resource`
/// resolved to, or an empty slice for the collection form and for an ask that
/// named nothing — the two say the same thing, and both mean every currency of
/// the target, now and later. Neither writes a row, which is what lets a
/// currency the target creates tomorrow be covered by a grant made today.
pub async fn request_grant(
    pool: &PgPool,
    application_id: i64,
    target: Target,
    scopes: &[String],
    resources: &[i64],
    expires_in: i64,
    now: OffsetDateTime,
) -> Result<GrantRequest> {
    for attempt in 0..16 {
        match request_grant_attempt(
            pool,
            application_id,
            target,
            scopes,
            resources,
            expires_in,
            now,
        )
        .await
        {
            Err(crate::Error::Database(error))
                if attempt < 15
                    && error.as_database_error().is_some_and(|error| {
                        error.is_unique_violation()
                            && error.constraint() == Some("grant_requests_pending_user_code_index")
                    }) =>
            {
                continue;
            }
            result => return result,
        }
    }
    unreachable!("the last attempt returns its result")
}

async fn request_grant_attempt(
    pool: &PgPool,
    application_id: i64,
    target: Target,
    scopes: &[String],
    resources: &[i64],
    expires_in: i64,
    now: OffsetDateTime,
) -> Result<GrantRequest> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let mut tx = pool.begin().await?;

    sqlx::query!(
        "SELECT id FROM applications WHERE id = $1 FOR NO KEY UPDATE",
        application_id
    )
    .fetch_one(&mut *tx)
    .await?;

    // Serialize retries for this application without blocking foreign-key reads
    // made by a concurrent approval. Reuse only the exact live permission set.
    sqlx::query!(
        "DELETE FROM grant_requests
          WHERE application_id = $1 AND status = 'pending'
            AND guild_id IS NOT DISTINCT FROM $2
            AND discord_id IS NOT DISTINCT FROM $3
            AND inserted_at + make_interval(secs => expires_in) <= $4",
        application_id,
        target.guild(),
        target.user(),
        at
    )
    .execute(&mut *tx)
    .await?;

    let row = sqlx::query!(
        r#"WITH existing AS (
           SELECT * FROM grant_requests WHERE application_id = $1
             AND guild_id IS NOT DISTINCT FROM $2 AND discord_id IS NOT DISTINCT FROM $3
             AND status = 'pending' AND scopes @> $4 AND scopes <@ $4
             AND resources @> $5 AND resources <@ $5
           ORDER BY id LIMIT 1
         ), inserted AS (
           INSERT INTO grant_requests (application_id, guild_id, discord_id, scopes, resources, device_code, user_code, expires_in, inserted_at, updated_at)
           SELECT $1, $2, $3, $4, $5, gen_random_uuid(), substring(replace(gen_random_uuid()::text, '-', '') from 1 for 8), $6, $7, $7
           WHERE NOT EXISTS (SELECT 1 FROM existing) RETURNING *
         ) SELECT id AS "id!", grant_id, guild_id, discord_id, scopes AS "scopes!: Vec<String>", resources AS "resources!: Vec<i64>",
                   device_code AS "device_code!: Uuid",
                   user_code AS "user_code!: String", status AS "status!: String",
                   GREATEST(0, expires_in - EXTRACT(EPOCH FROM ($7 - inserted_at))::bigint) AS "expires_in!: i64"
           FROM (SELECT * FROM existing UNION ALL SELECT * FROM inserted) chosen"#,
        application_id,
        target.guild(),
        target.user(),
        scopes,
        resources,
        expires_in,
        at
    )
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(GrantRequest {
        id: row.id,
        grant_id: row.grant_id,
        application_id,
        target: Target::of(row.guild_id, row.discord_id).ok_or(sqlx::Error::RowNotFound)?,
        scopes: row.scopes,
        resources: row.resources,
        device_code: row.device_code,
        user_code: row.user_code,
        status: row.status,
        expires_in: row.expires_in,
    })
}

/// A request as the application that made it reads it: the codes it polls with
/// and shows, the scopes it asked for, the currencies it asked for, who it is
/// put to, and how long it lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRequest {
    pub id: i64,
    pub grant_id: Option<i64>,
    pub application_id: i64,
    pub target: Target,
    pub scopes: Vec<String>,
    /// The currency ids the ask named, or empty for every currency of the target.
    pub resources: Vec<i64>,
    pub device_code: Uuid,
    pub user_code: String,
    pub status: String,
    pub expires_in: i64,
}

/// What an application has asked for, answered or not.
pub async fn requests_of(pool: &PgPool, application_id: i64) -> Result<Vec<GrantRequest>> {
    let rows = sqlx::query!(
        r#"SELECT id, grant_id, application_id, guild_id, discord_id, scopes AS "scopes!", resources AS "resources!",
                  device_code AS "device_code!: Uuid", user_code AS "user_code!",
                  status AS "status!", expires_in AS "expires_in!"
             FROM grant_requests
            WHERE application_id = $1
            ORDER BY id"#,
        application_id
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(GrantRequest {
                id: row.id,
                grant_id: row.grant_id,
                application_id: row.application_id,
                target: Target::of(row.guild_id, row.discord_id).ok_or(sqlx::Error::RowNotFound)?,
                scopes: row.scopes,
                resources: row.resources,
                device_code: row.device_code,
                user_code: row.user_code,
                status: row.status,
                expires_in: row.expires_in,
            })
        })
        .collect()
}

/// The target's answer to a request: the status, and for a yes the grant itself.
///
/// The grant is written from the ask's own scopes, never from anything the
/// caller names: an approval that granted something else would be a permission
/// nobody asked for. Its currencies travel the same way, for the same reason:
/// an approval is of the ask, so what this target could not choose is what it
/// could not widen. There is no no — `None` is a request that is not this
/// target's, not there, expired, or already answered, and all four are answered
/// the same way, because an unapproved ask simply stays pending until it dies.
///
/// The answer carries the currencies the ask named as well as the application,
/// because the screen that shows the decision has to name what was approved
/// rather than look it up again.
///
/// Whichever kind of target it is, the code has to be *that target's*: a guild
/// administrator answering a person's ask, or a person answering a guild's, is
/// the same nothing as a code nobody ever issued.
///
/// One transaction, because a request answered yes that granted nothing is a
/// state the application reads as approval and acts on.
pub async fn decide_request(
    pool: &PgPool,
    user_code: &str,
    target: Target,
    now: OffsetDateTime,
) -> Result<Option<Decided>> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    let mut tx = pool.begin().await?;

    let decided = sqlx::query!(
        r#"UPDATE grant_requests
              SET status = 'approved', updated_at = $4
            WHERE user_code = $1
              AND guild_id IS NOT DISTINCT FROM $2
              AND discord_id IS NOT DISTINCT FROM $3
              AND status = 'pending'
              AND inserted_at + make_interval(secs => expires_in) > $4
         RETURNING id, application_id, scopes AS "scopes!", resources AS "resources!""#,
        user_code,
        target.guild(),
        target.user(),
        at
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(decided) = decided else {
        return Ok(None);
    };

    let grant_id = write_grant(
        &mut tx,
        decided.application_id,
        target,
        &decided.scopes,
        &decided.resources,
        now,
        true,
    )
    .await?;
    sqlx::query!(
        "UPDATE grant_requests SET grant_id = $2 WHERE id = $1",
        decided.id,
        grant_id
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Some(Decided {
        application_id: decided.application_id,
        resources: decided.resources,
    }))
}

/// What an approval wrote, as the screen that shows the decision reads it: the
/// application, and the currencies the ask named.
///
/// The currencies are the ask's own — the same rows [`write_grant`] wrote — so
/// the screen names what was approved rather than looking it up again. Empty is
/// every currency of the target, which is what the collection form of
/// RFC 8707's `resource` and an ask that named nothing both mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decided {
    pub application_id: i64,
    pub resources: Vec<i64>,
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
        r#"SELECT id, grant_id, application_id, guild_id, discord_id, scopes AS "scopes!", resources AS "resources!",
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

    let Some(row) = row else {
        return Ok(None);
    };

    Ok(Some(GrantRequest {
        id: row.id,
        grant_id: row.grant_id,
        application_id: row.application_id,
        target: Target::of(row.guild_id, row.discord_id).ok_or(sqlx::Error::RowNotFound)?,
        scopes: row.scopes,
        resources: row.resources,
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

/// A pending request shown to its target before consent. The request id
/// binds the button to this exact request, not a user code that may be reused.
#[derive(Debug, Clone)]
pub struct Review {
    pub request_id: i64,
    pub application_id: i64,
    pub client_id: String,
    pub client_name: Option<String>,
    pub bot_discord_id: Option<i64>,
    pub target: Target,
    pub scopes: Vec<String>,
    pub resources: Vec<i64>,
}

/// Resolve only requests the caller is allowed to review. `guild` must already
/// have been authorized by the interaction's administrator permission.
pub async fn review_requests(
    pool: &PgPool,
    code: Option<&str>,
    request_id: Option<i64>,
    user: i64,
    guild: Option<i64>,
    now: OffsetDateTime,
) -> Result<Vec<Review>> {
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();
    let rows = sqlx::query!(
        r#"SELECT r.id AS request_id, r.application_id,
                  a.client_id::text AS "client_id!", a.client_name, r.guild_id, r.discord_id,
                  bot.discord_id AS bot_discord_id,
                  r.scopes AS "scopes!", r.resources AS "resources!"
             FROM grant_requests r JOIN applications a ON a.id = r.application_id
             LEFT JOIN users bot ON bot.application_id = a.id
            WHERE ((r.user_code = $1 AND $2::bigint IS NULL) OR r.id = $2)
              AND (r.discord_id = $3 OR r.guild_id = $4)
              AND r.status = 'pending'
              AND r.inserted_at + make_interval(secs => r.expires_in) > $5
            ORDER BY r.id"#,
        code,
        request_id,
        user,
        guild,
        at
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Review {
                request_id: row.request_id,
                application_id: row.application_id,
                client_id: row.client_id,
                client_name: row.client_name,
                bot_discord_id: row.bot_discord_id,
                target: Target::of(row.guild_id, row.discord_id).ok_or(sqlx::Error::RowNotFound)?,
                scopes: row.scopes,
                resources: row.resources,
            })
        })
        .collect()
}

/// Confirm only the exact immutable request shown by the review screen. The
/// caller must reauthenticate the target on this button press.
pub async fn approve_review(
    pool: &PgPool,
    review: &Review,
    now: OffsetDateTime,
) -> Result<Option<i64>> {
    let mut tx = pool.begin().await?;
    let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();
    let changed = sqlx::query!(
        "UPDATE grant_requests SET status = 'approved', updated_at = $4
          WHERE id = $1 AND guild_id IS NOT DISTINCT FROM $2
            AND discord_id IS NOT DISTINCT FROM $3 AND status = 'pending'
            AND inserted_at + make_interval(secs => expires_in) > $4
            AND scopes = $5 AND resources = $6 AND application_id = $7",
        review.request_id,
        review.target.guild(),
        review.target.user(),
        at,
        &review.scopes,
        &review.resources,
        review.application_id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Ok(None);
    }
    let grant_id = write_grant(
        &mut tx,
        review.application_id,
        review.target,
        &review.scopes,
        &review.resources,
        now,
        true,
    )
    .await?;
    sqlx::query!(
        "UPDATE grant_requests SET grant_id = $2 WHERE id = $1",
        review.request_id,
        grant_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(grant_id))
}

#[derive(Debug)]
pub struct PersonalApplication {
    pub grant_id: i64,
    pub client_id: String,
    pub client_name: Option<String>,
    pub bot_discord_id: Option<i64>,
    pub scopes: Vec<String>,
    pub resources: Vec<i64>,
}

/// A bounded page plus its count; the UI clamps pages after concurrent revocation.
pub async fn personal_applications(
    pool: &PgPool,
    user: i64,
    page: i64,
    limit: i64,
) -> Result<(Vec<PersonalApplication>, i64, i64)> {
    let total = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM grants WHERE discord_id = $1",
        user
    )
    .fetch_one(pool)
    .await?;
    let last = ((total + limit - 1) / limit).max(1);
    let page = page.clamp(1, last);
    let rows = sqlx::query!(
        r#"SELECT g.id AS grant_id, a.client_id::text AS "client_id!", a.client_name,
                  bot.discord_id AS bot_discord_id,
                  ARRAY(SELECT scope::text FROM grant_scopes WHERE grant_id = g.id ORDER BY scope::text) AS "scopes!",
                  ARRAY(SELECT currency_id FROM grant_resources WHERE grant_id = g.id ORDER BY currency_id) AS "resources!"
             FROM grants g JOIN applications a ON a.id = g.application_id
             LEFT JOIN users bot ON bot.application_id = a.id
            WHERE g.discord_id = $1 ORDER BY g.inserted_at DESC, g.id DESC LIMIT $2 OFFSET $3"#,
        user, limit, (page - 1) * limit
    ).fetch_all(pool).await?;
    Ok((
        rows.into_iter()
            .map(|row| PersonalApplication {
                grant_id: row.grant_id,
                client_id: row.client_id,
                client_name: row.client_name,
                bot_discord_id: row.bot_discord_id,
                scopes: row.scopes,
                resources: row.resources,
            })
            .collect(),
        total,
        page,
    ))
}

/// An existing grant is not an approval request and cannot be confirmed again.
#[derive(Debug)]
pub struct GrantDetails {
    pub grant_id: i64,
    pub application_id: i64,
    pub client_id: String,
    pub client_name: Option<String>,
    pub bot_discord_id: Option<i64>,
    pub target: Target,
    pub scopes: Vec<String>,
    pub resources: Vec<i64>,
}

/// Read one grant only for its owner or an administrator of its guild.
pub async fn grant_details(
    pool: &PgPool,
    id: i64,
    user: i64,
    guild: Option<i64>,
) -> Result<Option<GrantDetails>> {
    let row = sqlx::query!(r#"SELECT g.id, g.application_id AS "application_id!", g.guild_id, g.discord_id,
        a.client_id::text AS "client_id!", a.client_name, bot.discord_id AS bot_discord_id,
        ARRAY(SELECT scope::text FROM grant_scopes WHERE grant_id = g.id ORDER BY scope::text) AS "scopes!",
        ARRAY(SELECT currency_id FROM grant_resources WHERE grant_id = g.id ORDER BY currency_id) AS "resources!"
        FROM grants g JOIN applications a ON a.id = g.application_id
        LEFT JOIN users bot ON bot.application_id = a.id
        WHERE g.id = $1 AND (g.discord_id = $2 OR g.guild_id = $3)"#, id, user, guild).fetch_optional(pool).await?;
    row.map(|r| {
        Ok(GrantDetails {
            grant_id: r.id,
            application_id: r.application_id,
            client_id: r.client_id,
            client_name: r.client_name,
            bot_discord_id: r.bot_discord_id,
            target: Target::of(r.guild_id, r.discord_id).ok_or(sqlx::Error::RowNotFound)?,
            scopes: r.scopes,
            resources: r.resources,
        })
    })
    .transpose()
}

/// Delete exactly one owned grant; its access and refresh tokens cascade.
pub async fn revoke_one(pool: &PgPool, id: i64, user: i64, guild: Option<i64>) -> Result<bool> {
    Ok(sqlx::query!(
        "DELETE FROM grants WHERE id = $1 AND (discord_id = $2 OR guild_id = $3)",
        id,
        user,
        guild
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}
