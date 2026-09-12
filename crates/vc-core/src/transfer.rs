use sqlx::PgConnection;

use crate::model::utc_now;

#[derive(Debug)]
pub enum TransferError {
    NotFoundCurrency,
    NotFoundSenderAsset,
    NotEnoughAmount,
    Database(sqlx::Error),
}

/// `VirtualCrypto.Money.Query.Asset.Transfer.transfer/4`.
///
/// Runs inside the caller's transaction: the sender's asset is locked with
/// `FOR UPDATE`, the receiver is upserted, the sender is decremented, and the
/// payment is recorded. The `assets` trigger deletes the sender's row when the
/// balance reaches zero, so a zero balance is the absence of a row.
pub async fn transfer(
    conn: &mut PgConnection,
    sender_id: i32,
    receiver_id: i32,
    amount: i64,
    unit: &str,
) -> Result<(), TransferError> {
    let currency_id = sqlx::query_scalar!("SELECT id FROM currencies WHERE unit = $1", unit)
        .fetch_optional(&mut *conn)
        .await
        .map_err(TransferError::Database)?
        .ok_or(TransferError::NotFoundCurrency)?;

    let sender = sqlx::query!(
        "SELECT amount FROM assets
          WHERE user_id = $1 AND currency_id = $2
            FOR UPDATE",
        i64::from(sender_id),
        currency_id
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(TransferError::Database)?;

    let sender_amount = sender
        .and_then(|row| row.amount)
        .ok_or(TransferError::NotFoundSenderAsset)?;

    if sender_amount < amount {
        return Err(TransferError::NotEnoughAmount);
    }

    let now = utc_now();

    sqlx::query!(
        "INSERT INTO assets (user_id, currency_id, amount, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)
         ON CONFLICT (user_id, currency_id)
         DO UPDATE SET amount = assets.amount + EXCLUDED.amount,
                       updated_at = EXCLUDED.updated_at",
        i64::from(receiver_id),
        currency_id,
        amount,
        now
    )
    .execute(&mut *conn)
    .await
    .map_err(TransferError::Database)?;

    sqlx::query!(
        "UPDATE assets
            SET amount = amount - $1, updated_at = $2
          WHERE user_id = $3 AND currency_id = $4",
        amount,
        now,
        i64::from(sender_id),
        currency_id
    )
    .execute(&mut *conn)
    .await
    .map_err(TransferError::Database)?;

    sqlx::query!(
        "INSERT INTO currency_payment_histories
             (amount, sender_id, receiver_id, currency_id, \"time\", inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $5, $5)",
        amount,
        i64::from(sender_id),
        i64::from(receiver_id),
        currency_id,
        now
    )
    .execute(&mut *conn)
    .await
    .map_err(TransferError::Database)?;

    Ok(())
}
