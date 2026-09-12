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
