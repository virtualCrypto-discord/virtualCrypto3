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
