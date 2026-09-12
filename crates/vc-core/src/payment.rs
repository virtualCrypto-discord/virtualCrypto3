use sqlx::PgPool;

use crate::transfer::TransferError;

#[derive(Debug)]
pub enum PayError {
    InvalidAmount,
    NotFoundCurrency,
    NotFoundSenderAsset,
    NotEnoughAmount,
    Database(sqlx::Error),
}

/// `Money.pay/1`: send currency from one account to a discord user, creating the
/// receiver's account when they have none.
///
/// The currency is resolved before the receiver, matching the order inside
/// `Query.Asset.Transfer.transfer/4`, so an unknown unit is reported before any
/// user is created.
pub async fn pay(
    pool: &PgPool,
    sender_id: i32,
    receiver_discord_id: i64,
    unit: &str,
    amount: i64,
) -> Result<(), PayError> {
    let mut tx = pool.begin().await.map_err(PayError::Database)?;

    let known = sqlx::query_scalar!("SELECT id FROM currencies WHERE unit = $1", unit)
        .fetch_optional(&mut *tx)
        .await
        .map_err(PayError::Database)?;

    if known.is_none() {
        return Err(PayError::NotFoundCurrency);
    }

    let receiver = crate::user::insert_if_not_exists(&mut tx, receiver_discord_id)
        .await
        .map_err(PayError::Database)?;

    crate::transfer::transfer(&mut tx, sender_id, receiver.id, amount, unit)
        .await
        .map_err(|error| match error {
            TransferError::InvalidAmount => PayError::InvalidAmount,
            TransferError::NotFoundCurrency => PayError::NotFoundCurrency,
            TransferError::NotFoundSenderAsset => PayError::NotFoundSenderAsset,
            TransferError::NotEnoughAmount => PayError::NotEnoughAmount,
            TransferError::Database(error) => PayError::Database(error),
        })?;

    tx.commit().await.map_err(PayError::Database)?;

    Ok(())
}
