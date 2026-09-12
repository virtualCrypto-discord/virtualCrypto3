use sqlx::PgConnection;
use sqlx::PgPool;
use time::PrimitiveDateTime;

use crate::error::Result;
use crate::model::{DiscordAuth, User};

pub async fn find_by_id(pool: &PgPool, id: i32) -> Result<Option<User>> {
    let user = sqlx::query_as!(
        User,
        "SELECT id, discord_id, status FROM users WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await?;

    Ok(user)
}

pub async fn find_by_discord_id(pool: &PgPool, discord_id: i64) -> Result<Option<User>> {
    let user = sqlx::query_as!(
        User,
        "SELECT id, discord_id, status FROM users WHERE discord_id = $1",
        discord_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(user)
}

/// `VirtualCrypto.User.insert_user_if_not_exists/1`: create the account for a
/// discord id when it is missing. Claim creation resolves its payer this way, so
/// creating a claim can add a user row.
pub async fn insert_if_not_exists(
    conn: &mut PgConnection,
    discord_id: i64,
) -> std::result::Result<User, sqlx::Error> {
    let inserted = sqlx::query_as!(
        User,
        "INSERT INTO users (discord_id, status, inserted_at, updated_at)
         VALUES ($1, 0, $2, $2)
         ON CONFLICT (discord_id) DO NOTHING
         RETURNING id, discord_id, status",
        discord_id,
        crate::model::utc_now()
    )
    .fetch_optional(&mut *conn)
    .await?;

    match inserted {
        Some(user) => Ok(user),
        None => {
            sqlx::query_as!(
                User,
                "SELECT id, discord_id, status FROM users WHERE discord_id = $1",
                discord_id
            )
            .fetch_one(&mut *conn)
            .await
        }
    }
}

/// `UserResolver.resolve_id/1` for a discord id: the account's id, creating the
/// account when it is missing.
pub async fn resolve_discord_id(
    pool: &PgPool,
    discord_id: i64,
) -> std::result::Result<i32, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let user = insert_if_not_exists(&mut tx, discord_id).await?;
    tx.commit().await?;

    Ok(user.id)
}

/// Resolve several discord ids at once, creating the accounts that are missing.
///
/// `UserResolver.resolve_ids/1` does the same: a fixed number of statements
/// regardless of how many ids are asked for, rather than one per id.
pub async fn resolve_ids(
    conn: &mut PgConnection,
    discord_ids: &[i64],
) -> std::result::Result<std::collections::HashMap<i64, i32>, sqlx::Error> {
    if discord_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let now = crate::model::utc_now();
    sqlx::query!(
        "INSERT INTO users (discord_id, status, inserted_at, updated_at)
         SELECT t.discord_id, 0, $2, $2 FROM UNNEST($1::bigint[]) AS t(discord_id)
         ON CONFLICT (discord_id) DO NOTHING",
        discord_ids,
        now
    )
    .execute(&mut *conn)
    .await?;

    let rows = sqlx::query!(
        "SELECT id, discord_id FROM users WHERE discord_id = ANY($1)",
        discord_ids
    )
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|row| row.discord_id.map(|discord_id| (discord_id, row.id)))
        .collect())
}

pub async fn find_discord_auth(pool: &PgPool, discord_user_id: i64) -> Result<Option<DiscordAuth>> {
    let auth = sqlx::query_as!(
        DiscordAuth,
        "SELECT discord_user_id AS \"discord_user_id!\", token, refresh_token, updated_at
         FROM discord_users WHERE discord_user_id = $1",
        discord_user_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(auth)
}

/// Store a refreshed Discord authorization.
///
/// Deliberately leaves `updated_at` alone: the Elixir code updates this row with
/// `Ecto.Repo.update_all/2`, which does not apply timestamps. The captured
/// golden confirms `updated_at` is unchanged after a refresh.
pub async fn update_discord_token(
    pool: &PgPool,
    discord_user_id: i64,
    token: &str,
    refresh_token: Option<&str>,
    expires: PrimitiveDateTime,
) -> Result<()> {
    sqlx::query!(
        "UPDATE discord_users
            SET token = $1, refresh_token = $2, expires = $3
          WHERE discord_user_id = $4",
        token,
        refresh_token,
        expires,
        discord_user_id
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// `VirtualCrypto.DiscordAuth.insert_user/4`: record the authorization a login
/// just obtained, and give back the account it belongs to.
///
/// A transaction, as the Elixir side is: the account and its authorization are
/// created together, and a half-written login would leave an account that can
/// neither be used nor retried.
///
/// `updated_at` is set to now on conflict, which is load-bearing rather than
/// tidy — the refresh window is measured from it, so a login is what starts the
/// seven days before the next refresh.
pub async fn insert_user(
    pool: &PgPool,
    discord_user_id: i64,
    token: &str,
    refresh_token: Option<&str>,
    expires: PrimitiveDateTime,
) -> Result<User> {
    let mut tx = pool.begin().await?;

    let user = insert_if_not_exists(&mut tx, discord_user_id).await?;

    sqlx::query!(
        "INSERT INTO discord_users (discord_user_id, token, refresh_token, expires, updated_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (discord_user_id)
         DO UPDATE SET token = $2, refresh_token = $3, expires = $4, updated_at = $5",
        discord_user_id,
        token,
        refresh_token,
        expires,
        crate::model::utc_now()
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(user)
}
