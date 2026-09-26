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
    loop {
        if let Some(user) = insert_account_if_missing(conn, discord_id).await? {
            return Ok(user);
        }
        // Keep the resolved id alive until the caller commits its payment,
        // claim or issuance. A binding may replace it while this read waits;
        // in that case resolve the Discord id again using a fresh snapshot.
        if let Some(user) = sqlx::query_as!(
            User,
            "SELECT id, discord_id, status FROM users WHERE discord_id = $1 FOR KEY SHARE",
            discord_id
        )
        .fetch_optional(&mut *conn)
        .await?
        {
            return Ok(user);
        }
    }
}

async fn insert_account_if_missing(
    conn: &mut PgConnection,
    discord_id: i64,
) -> std::result::Result<Option<User>, sqlx::Error> {
    sqlx::query_as!(
        User,
        "INSERT INTO users (discord_id, status, inserted_at, updated_at)
         VALUES ($1, 0, $2, $2)
         ON CONFLICT (discord_id) DO NOTHING
         RETURNING id, discord_id, status",
        discord_id,
        crate::model::utc_now()
    )
    .fetch_optional(conn)
    .await
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

    loop {
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
            "SELECT id, discord_id FROM users WHERE discord_id = ANY($1) ORDER BY id FOR KEY SHARE",
            discord_ids
        )
        .fetch_all(&mut *conn)
        .await?;
        let resolved: std::collections::HashMap<_, _> = rows
            .into_iter()
            .filter_map(|row| row.discord_id.map(|discord_id| (discord_id, row.id)))
            .collect();
        if discord_ids.iter().all(|id| resolved.contains_key(id)) {
            return Ok(resolved);
        }
        // A binding deleted or reassigned a row while the SELECT waited.
        // Resolve again, recreating the old bot's account after a rebind.
    }
}

pub async fn find_discord_auth(pool: &PgPool, discord_user_id: i64) -> Result<Option<DiscordAuth>> {
    let auth = sqlx::query_as!(
        DiscordAuth,
        "SELECT discord_user_id AS \"discord_user_id!\", token, refresh_token, expires, updated_at
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
/// golden confirms `updated_at` is unchanged after a refresh. Subsequent refresh
/// decisions use the new `expires`, not this unchanged timestamp.
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
/// Both timestamps are set on conflict, because Ecto's `on_conflict:
/// :replace_all` writes every field the struct carries and Ecto had filled in
/// both. The supplied `expires` determines when the authorization needs refreshing.
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
        "INSERT INTO discord_users
             (discord_user_id, token, refresh_token, expires, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $5)
         ON CONFLICT (discord_user_id)
         DO UPDATE SET token = $2, refresh_token = $3, expires = $4,
                       inserted_at = $5, updated_at = $5",
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

/// The user an application acts as: the row whose `application_id` points at it.
///
/// `Auth.get_application_user_id_by_client_id/2` does this and the secret check
/// in one query, which makes the secret comparison a database's. Here the secret
/// has already been compared in constant time, so this is only the link.
pub async fn application_user_id(
    pool: &PgPool,
    application_id: i64,
) -> std::result::Result<Option<i32>, sqlx::Error> {
    let found = sqlx::query_scalar!(
        "SELECT id FROM users WHERE application_id = $1",
        application_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(found)
}

/// The application an account acts for, if it acts for one.
///
/// `users.application_id` is the link from an account to the application it
/// belongs to. A user without one has nowhere to be notified, which is the
/// notification path's `:nop` rather than an error.
pub async fn application_id(
    pool: &PgPool,
    user_id: i32,
) -> std::result::Result<Option<i64>, sqlx::Error> {
    let found = sqlx::query_scalar!("SELECT application_id FROM users WHERE id = $1", user_id)
        .fetch_optional(pool)
        .await?;

    // The column is nullable, so this is an `Option` of an `Option`: no such
    // user, or a user that is not anybody's application.
    Ok(found.flatten())
}

/// What binding a bot to an application ran into.
#[derive(Debug)]
pub enum BindError {
    /// The bot already belongs to another application.
    Taken,
    Database(sqlx::Error),
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindError::Taken => write!(f, "that bot is another application's"),
            BindError::Database(error) => write!(f, "{error}"),
        }
    }
}

/// `ConnectUser.set_discord_user_id/2`: an application's account, and the bot that
/// speaks for it.
///
/// Payments and claims can create the bot's ordinary account before its owner
/// connects it. Merge that account into the application's existing account, whose
/// id must remain stable for already-issued application tokens. Lock both accounts
/// in transfer order; all balances and references move in the same transaction.
pub async fn bind_bot(
    pool: &sqlx::PgPool,
    application_user_id: i32,
    bot_id: i64,
) -> std::result::Result<(), BindError> {
    loop {
        let mut tx = pool.begin().await.map_err(BindError::Database)?;
        // Materialize an absent account too, so concurrent first connections have
        // an existing row to arbitrate over rather than racing the final UPDATE.
        insert_account_if_missing(&mut tx, bot_id)
            .await
            .map_err(BindError::Database)?;
        let accounts = sqlx::query!(
            "SELECT id, discord_id, application_id, contract_id FROM users
              WHERE id = $1 OR discord_id = $2 ORDER BY id FOR UPDATE NOWAIT",
            application_user_id,
            bot_id
        )
        .fetch_all(&mut *tx)
        .await;
        let accounts = match accounts {
            Ok(accounts) => accounts,
            Err(error) if lock_unavailable(&error) => {
                // A writer can already hold a shared reference to one account
                // and need the other's transfer lock. Release both and retry
                // rather than holding the other half while waiting for it.
                tx.rollback().await.map_err(BindError::Database)?;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                continue;
            }
            Err(error) => return Err(BindError::Database(error)),
        };
        if !accounts
            .iter()
            .any(|account| account.id == application_user_id && account.application_id.is_some())
        {
            return Err(BindError::Database(sqlx::Error::RowNotFound));
        }
        let Some(source) = accounts
            .iter()
            .find(|account| account.discord_id == Some(bot_id))
        else {
            // Another connection merged the source while we waited for its lock.
            // A fresh transaction sees the account that now owns this Discord id.
            tx.rollback().await.map_err(BindError::Database)?;
            continue;
        };
        if source.id != application_user_id {
            if source.application_id.is_some() || source.contract_id.is_some() {
                return Err(BindError::Taken);
            }
            match merge_bot_account(&mut tx, application_user_id, source.id).await {
                Ok(()) => {}
                Err(error) if lock_unavailable(&error) => {
                    tx.rollback().await.map_err(BindError::Database)?;
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    continue;
                }
                Err(error) => return Err(BindError::Database(error)),
            }
        }
        sqlx::query!(
            "UPDATE users SET discord_id = $1, updated_at = now() WHERE id = $2",
            bot_id,
            application_user_id
        )
        .execute(&mut *tx)
        .await
        .map_err(BindError::Database)?;
        tx.commit().await.map_err(BindError::Database)?;
        return Ok(());
    }
}

fn lock_unavailable(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|error| error.code().as_deref() == Some("55P03"))
}

async fn merge_bot_account(
    conn: &mut PgConnection,
    destination: i32,
    source: i32,
) -> std::result::Result<(), sqlx::Error> {
    // Issuance locks a currency before resolving its receiver, and claim
    // transitions lock the claim before transferring money. Do not wait for
    // either while holding the account locks they may still need.
    sqlx::query!(
        "SELECT id FROM currencies WHERE id IN
             (SELECT currency_id FROM assets WHERE user_id IN ($1, $2))
         ORDER BY id FOR KEY SHARE NOWAIT",
        i64::from(source),
        i64::from(destination)
    )
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query!(
        "SELECT id FROM claims WHERE claimant_user_id = $1 OR payer_user_id = $1
         ORDER BY id FOR UPDATE NOWAIT",
        i64::from(source)
    )
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query!(
        "WITH moved AS (
             DELETE FROM assets WHERE user_id = $1 RETURNING currency_id, amount
         )
         INSERT INTO assets (user_id, currency_id, amount, inserted_at, updated_at)
         SELECT $2, currency_id, amount, now(), now() FROM moved
         ON CONFLICT (user_id, currency_id)
         DO UPDATE SET amount = assets.amount + EXCLUDED.amount, updated_at = now()",
        i64::from(source),
        i64::from(destination)
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "UPDATE currency_given_histories SET receiver_id = $2 WHERE receiver_id = $1",
        i64::from(source),
        i64::from(destination)
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "UPDATE currency_payment_histories
            SET sender_id = CASE WHEN sender_id = $1 THEN $2 ELSE sender_id END,
                receiver_id = CASE WHEN receiver_id = $1 THEN $2 ELSE receiver_id END
          WHERE sender_id = $1 OR receiver_id = $1",
        i64::from(source),
        i64::from(destination)
    )
    .execute(&mut *conn)
    .await?;
    // The composite foreign key is immediate: move the claim and its metadata
    // together in one statement so every reference is valid at statement end.
    sqlx::query!(
        "WITH moved AS (
             UPDATE claims
                SET claimant_user_id = CASE WHEN claimant_user_id = $1 THEN $2 ELSE claimant_user_id END,
                    payer_user_id = CASE WHEN payer_user_id = $1 THEN $2 ELSE payer_user_id END
              WHERE claimant_user_id = $1 OR payer_user_id = $1
              RETURNING id, claimant_user_id, payer_user_id
         )
         UPDATE claim_metadata m
            SET claimant_user_id = moved.claimant_user_id,
                payer_user_id = moved.payer_user_id,
                owner_user_id = CASE WHEN m.owner_user_id = $1 THEN $2 ELSE m.owner_user_id END
           FROM moved WHERE m.claim_id = moved.id",
        i64::from(source),
        i64::from(destination)
    )
    .execute(&mut *conn)
    .await?;
    // Preserve incoming mutes, as well as any preferences of the old account.
    // If both accounts were muted, the resulting preference is still one mute.
    sqlx::query!(
        "WITH moved AS (
             DELETE FROM mutes WHERE user_id = $1 OR muted_user_id = $1
             RETURNING user_id, currency_id, muted_user_id, inserted_at
         )
         INSERT INTO mutes (user_id, currency_id, muted_user_id, inserted_at)
         SELECT CASE WHEN user_id = $1 THEN $2 ELSE user_id END, currency_id,
                CASE WHEN muted_user_id = $1 THEN $2 ELSE muted_user_id END, inserted_at
           FROM moved
         ON CONFLICT DO NOTHING",
        source,
        destination
    )
    .execute(&mut *conn)
    .await?;
    // These keys belonged to the retired account, not to the application.
    sqlx::query!(
        "DELETE FROM payments_idempotency WHERE user_id = $1",
        i64::from(source)
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("DELETE FROM users WHERE id = $1", source)
        .execute(conn)
        .await?;
    Ok(())
}
