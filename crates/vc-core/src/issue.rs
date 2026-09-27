use sqlx::{PgConnection, PgPool};

use crate::model::utc_now;

#[derive(Debug)]
pub enum IssueError {
    NotFoundCurrency,
    NotEnoughAmount,
    InvalidAmount,
    Database(sqlx::Error),
}

/// What `Money.give/1` reports: how much was issued, and what the pool holds
/// afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issued {
    pub amount: i64,
    pub pool_amount: i64,
    pub unit: String,
}

/// `Money.give/1` through `Query.Issue.issue/3`: hand out currency from a guild's
/// pool.
///
/// The Elixir named the command `give` and the query it calls `issue`; this is one function,
/// named after the query.
///
/// `amount` of `None` is the `:all` a `/issue` without an amount asks for, which
/// issues whatever the pool holds. The currency is read with a lock, so the pool
/// check and the decrement cannot interleave with another issue.
pub async fn issue(
    pool: &PgPool,
    guild_id: i64,
    receiver_discord_id: i64,
    amount: Option<i64>,
) -> std::result::Result<Issued, IssueError> {
    let mut tx = pool.begin().await.map_err(IssueError::Database)?;

    let issued = issue_in(&mut tx, guild_id, receiver_discord_id, amount).await?;

    tx.commit().await.map_err(IssueError::Database)?;

    Ok(issued)
}

/// The same issue, on a transaction the caller owns — which is what lets the
/// idempotency layer's claim, this write and the stored answer be one commit.
///
/// The currency is read with a lock, so the pool check and the decrement cannot
/// interleave with another issue. NO KEY UPDATE still serializes issuers, but
/// lets transfers check their currency foreign keys while holding a balance
/// this issue may need. FOR UPDATE would make those operations deadlock.
pub async fn issue_in(
    tx: &mut PgConnection,
    guild_id: i64,
    receiver_discord_id: i64,
    amount: Option<i64>,
) -> std::result::Result<Issued, IssueError> {
    let currency = sqlx::query!(
        "SELECT id, unit, pool_amount FROM currencies WHERE guild_id = $1 FOR NO KEY UPDATE",
        guild_id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(IssueError::Database)?;

    let Some(currency) = currency else {
        return Err(IssueError::NotFoundCurrency);
    };

    let pool_amount = currency.pool_amount.unwrap_or(0);

    let amount = match amount {
        // `:all` issues the pool, which has to hold something to be worth a
        // history row.
        None if pool_amount > 0 => pool_amount,
        None => return Err(IssueError::NotEnoughAmount),
        Some(amount) if amount > 0 && pool_amount >= amount => amount,
        Some(amount) if amount > 0 => return Err(IssueError::NotEnoughAmount),
        Some(_) => return Err(IssueError::InvalidAmount),
    };

    let receiver = crate::user::insert_if_not_exists(&mut *tx, receiver_discord_id)
        .await
        .map_err(IssueError::Database)?;

    let now = utc_now();

    sqlx::query!(
        "INSERT INTO assets (user_id, currency_id, amount, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)
         ON CONFLICT (user_id, currency_id)
         DO UPDATE SET amount = assets.amount + EXCLUDED.amount,
                       updated_at = EXCLUDED.updated_at",
        i64::from(receiver.id),
        currency.id,
        amount,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(IssueError::Database)?;

    sqlx::query!(
        "UPDATE currencies SET pool_amount = pool_amount - $1 WHERE id = $2",
        amount,
        currency.id
    )
    .execute(&mut *tx)
    .await
    .map_err(IssueError::Database)?;

    sqlx::query!(
        "INSERT INTO currency_given_histories
             (amount, currency_id, \"time\", receiver_id, inserted_at, updated_at,
              receiver_balance_after, pool_balance_after)
         VALUES ($1, $2, $3, $4, $3, $3,
                 COALESCE((SELECT amount FROM assets WHERE user_id = $4 AND currency_id = $2), 0),
                 (SELECT pool_amount FROM currencies WHERE id = $2))",
        amount,
        currency.id,
        now,
        i64::from(receiver.id)
    )
    .execute(&mut *tx)
    .await
    .map_err(IssueError::Database)?;

    Ok(Issued {
        amount,
        pool_amount: pool_amount - amount,
        unit: currency.unit.unwrap_or_default(),
    })
}
