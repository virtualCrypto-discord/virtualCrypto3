use sqlx::PgPool;

use crate::model::utc_now;

#[derive(Debug)]
pub enum GiveError {
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
/// `amount` of `None` is the `:all` a `/give` without an amount asks for, which
/// issues whatever the pool holds. The currency is read with a lock, so the pool
/// check and the decrement cannot interleave with another issue.
pub async fn give(
    pool: &PgPool,
    guild_id: i64,
    receiver_discord_id: i64,
    amount: Option<i64>,
) -> std::result::Result<Issued, GiveError> {
    let mut tx = pool.begin().await.map_err(GiveError::Database)?;

    let currency = sqlx::query!(
        "SELECT id, unit, pool_amount FROM currencies WHERE guild_id = $1 FOR UPDATE",
        guild_id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(GiveError::Database)?;

    let Some(currency) = currency else {
        return Err(GiveError::NotFoundCurrency);
    };

    let pool_amount = currency.pool_amount.unwrap_or(0);

    let amount = match amount {
        // `:all` issues the pool, which has to hold something to be worth a
        // history row.
        None if pool_amount > 0 => pool_amount,
        None => return Err(GiveError::NotEnoughAmount),
        Some(amount) if amount > 0 && pool_amount >= amount => amount,
        Some(amount) if amount > 0 => return Err(GiveError::NotEnoughAmount),
        Some(_) => return Err(GiveError::InvalidAmount),
    };

    let receiver = crate::user::insert_if_not_exists(&mut tx, receiver_discord_id)
        .await
        .map_err(GiveError::Database)?;

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
    .map_err(GiveError::Database)?;

    sqlx::query!(
        "UPDATE currencies SET pool_amount = pool_amount - $1 WHERE id = $2",
        amount,
        currency.id
    )
    .execute(&mut *tx)
    .await
    .map_err(GiveError::Database)?;

    sqlx::query!(
        "INSERT INTO currency_given_histories
             (amount, currency_id, \"time\", receiver_id, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $3, $3)",
        amount,
        currency.id,
        now,
        i64::from(receiver.id)
    )
    .execute(&mut *tx)
    .await
    .map_err(GiveError::Database)?;

    tx.commit().await.map_err(GiveError::Database)?;

    Ok(Issued {
        amount,
        pool_amount: pool_amount - amount,
        unit: currency.unit.unwrap_or_default(),
    })
}
