use std::collections::BTreeMap;

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

/// `Money.pay/1` when both sides come from Discord, which is the `pay` slash
/// command: `Query.Asset.Transfer.transfer/4` resolves its arguments with
/// `UserResolver.resolve_ids/1`, so a discord id that has no account gets one.
///
/// The currency is checked before the accounts are resolved, keeping the order
/// [`pay`] uses, so an unknown unit is reported without creating anyone.
pub async fn pay_from_discord(
    pool: &PgPool,
    sender_discord_id: i64,
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

    let ids = crate::user::resolve_ids(&mut tx, &[sender_discord_id, receiver_discord_id])
        .await
        .map_err(PayError::Database)?;

    crate::transfer::transfer(
        &mut tx,
        ids[&sender_discord_id],
        ids[&receiver_discord_id],
        amount,
        unit,
    )
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

/// One entry of a bulk payment.
#[derive(Debug, Clone)]
pub struct BulkPayment {
    pub unit: String,
    pub receiver_discord_id: i64,
    pub amount: i64,
}

/// `Money.create_payments/3`: resolve the receivers, then hand the batch to
/// [`crate::transfer::transfer_bulk`], which writes it in a fixed number of
/// statements.
///
/// The currency is checked before the receivers are resolved, so an unknown unit
/// is reported without creating any accounts.
pub async fn pay_bulk(
    pool: &PgPool,
    sender_id: i32,
    payments: &[BulkPayment],
) -> Result<(), PayError> {
    if payments.is_empty() {
        return Ok(());
    }

    if payments.iter().any(|payment| payment.amount <= 0) {
        return Err(PayError::InvalidAmount);
    }

    let mut tx = pool.begin().await.map_err(PayError::Database)?;

    let mut units: Vec<String> = payments
        .iter()
        .map(|payment| payment.unit.clone())
        .collect();
    units.sort();
    units.dedup();

    let known = sqlx::query!(
        "SELECT id, unit FROM currencies WHERE unit = ANY($1)",
        &units
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(PayError::Database)?;

    let mut currency_of: BTreeMap<String, i64> = BTreeMap::new();
    for row in known {
        if let Some(unit) = row.unit {
            currency_of.insert(unit, row.id);
        }
    }

    if currency_of.len() != units.len() {
        return Err(PayError::NotFoundCurrency);
    }

    let mut discord_ids: Vec<i64> = payments
        .iter()
        .map(|payment| payment.receiver_discord_id)
        .collect();
    discord_ids.sort();
    discord_ids.dedup();

    let receivers = crate::user::resolve_ids(&mut tx, &discord_ids)
        .await
        .map_err(PayError::Database)?;

    let entries: Vec<crate::transfer::BulkEntry> = payments
        .iter()
        .map(|payment| {
            (
                payment.unit.clone(),
                receivers[&payment.receiver_discord_id],
                payment.amount,
            )
        })
        .collect();

    crate::transfer::transfer_bulk(&mut tx, sender_id, &entries)
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
