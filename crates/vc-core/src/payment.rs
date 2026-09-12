use std::collections::BTreeMap;

use sqlx::PgPool;

use crate::model::utc_now;
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

/// `Money.create_payments/3` through `Query.Asset.Transfer.transfer_bulk/3`.
///
/// The whole batch is written with a fixed number of statements — resolve the
/// currencies, resolve the receivers, lock the sender's rows, then one upsert,
/// one decrement and one history insert — rather than one transfer per entry.
/// The per-currency totals are checked against the locked balances before
/// anything is written, so an over-committed batch fails as a whole.
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

    // Net the batch per (currency, receiver): the same pair may appear twice.
    let mut totals: BTreeMap<(i64, i32), i64> = BTreeMap::new();
    for payment in payments {
        let currency_id = currency_of[&payment.unit];
        let receiver_id = receivers[&payment.receiver_discord_id];
        *totals.entry((currency_id, receiver_id)).or_insert(0) += payment.amount;
    }

    let mut per_currency: BTreeMap<i64, i64> = BTreeMap::new();
    for ((currency_id, _), amount) in &totals {
        *per_currency.entry(*currency_id).or_insert(0) += amount;
    }

    let currency_ids: Vec<i64> = per_currency.keys().copied().collect();

    let locked = sqlx::query!(
        "SELECT currency_id AS \"currency_id!\", amount FROM assets
          WHERE user_id = $1 AND currency_id = ANY($2)
            FOR UPDATE",
        i64::from(sender_id),
        &currency_ids
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(PayError::Database)?;

    let balances: BTreeMap<i64, i64> = locked
        .into_iter()
        .map(|row| (row.currency_id, row.amount.unwrap_or(0)))
        .collect();

    for (currency_id, sent) in &per_currency {
        if balances.get(currency_id).copied().unwrap_or(0) < *sent {
            return Err(PayError::NotEnoughAmount);
        }
    }

    let now = utc_now();

    let receiver_ids: Vec<i64> = totals.keys().map(|(_, id)| i64::from(*id)).collect();
    let currency_ids: Vec<i64> = totals.keys().map(|(id, _)| *id).collect();
    let amounts: Vec<i64> = totals.values().copied().collect();

    sqlx::query!(
        "INSERT INTO assets (user_id, currency_id, amount, inserted_at, updated_at)
         SELECT t.user_id, t.currency_id, t.amount, $4, $4
           FROM UNNEST($1::bigint[], $2::bigint[], $3::bigint[])
             AS t(user_id, currency_id, amount)
         ON CONFLICT (user_id, currency_id)
         DO UPDATE SET amount = assets.amount + EXCLUDED.amount,
                       updated_at = EXCLUDED.updated_at",
        &receiver_ids,
        &currency_ids,
        &amounts,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(PayError::Database)?;

    let delta_currencies: Vec<i64> = per_currency.keys().copied().collect();
    let deltas: Vec<i64> = per_currency.values().copied().collect();

    sqlx::query!(
        "UPDATE assets
            SET amount = assets.amount - t.delta, updated_at = $3
           FROM UNNEST($1::bigint[], $2::bigint[]) AS t(currency_id, delta)
          WHERE assets.user_id = $4 AND assets.currency_id = t.currency_id",
        &delta_currencies,
        &deltas,
        now,
        i64::from(sender_id)
    )
    .execute(&mut *tx)
    .await
    .map_err(PayError::Database)?;

    let history_amounts: Vec<i64> = totals.values().copied().collect();
    let history_receivers: Vec<i64> = totals.keys().map(|(_, id)| i64::from(*id)).collect();
    let history_currencies: Vec<i64> = totals.keys().map(|(id, _)| *id).collect();

    sqlx::query!(
        "INSERT INTO currency_payment_histories
             (amount, sender_id, receiver_id, currency_id, \"time\", inserted_at, updated_at)
         SELECT t.amount, $4, t.receiver_id, t.currency_id, $5, $5, $5
           FROM UNNEST($1::bigint[], $2::bigint[], $3::bigint[])
             AS t(amount, receiver_id, currency_id)",
        &history_amounts,
        &history_receivers,
        &history_currencies,
        i64::from(sender_id),
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(PayError::Database)?;

    tx.commit().await.map_err(PayError::Database)?;

    Ok(())
}
