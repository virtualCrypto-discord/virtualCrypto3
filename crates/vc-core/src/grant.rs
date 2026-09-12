//! OAuth2 grants, and the tokens that belong to them.

use sqlx::PgPool;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

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
