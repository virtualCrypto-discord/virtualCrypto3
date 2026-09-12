use std::collections::BTreeMap;

use sqlx::PgConnection;

use crate::model::utc_now;

#[derive(Debug)]
pub enum TransferError {
    InvalidAmount,
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
    if amount <= 0 {
        return Err(TransferError::InvalidAmount);
    }

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

/// One entry of a bulk transfer: the currency's unit, the receiver's account id
/// and the amount.
pub type BulkEntry = (String, i32, i64);

/// `Query.Asset.Transfer.transfer_bulk/3`: one sender paying several receivers.
///
/// The batch costs a fixed number of statements — resolve the currencies, lock
/// the sender's rows, then one upsert, one decrement and one history insert —
/// rather than one transfer per entry. Entries naming the same
/// (currency, receiver) are netted first, and the per-currency totals are
/// checked against the locked balances before anything is written, so an
/// over-committed batch fails as a whole.
pub async fn transfer_bulk(
    conn: &mut PgConnection,
    sender_id: i32,
    entries: &[BulkEntry],
) -> std::result::Result<(), TransferError> {
    if entries.is_empty() {
        return Ok(());
    }

    if entries.iter().any(|(_, _, amount)| *amount <= 0) {
        return Err(TransferError::InvalidAmount);
    }

    let mut units: Vec<String> = entries.iter().map(|(unit, _, _)| unit.clone()).collect();
    units.sort();
    units.dedup();

    let known = sqlx::query!(
        "SELECT id, unit FROM currencies WHERE unit = ANY($1)",
        &units
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(TransferError::Database)?;

    let mut currency_of: BTreeMap<String, i64> = BTreeMap::new();
    for row in known {
        if let Some(unit) = row.unit {
            currency_of.insert(unit, row.id);
        }
    }

    if currency_of.len() != units.len() {
        return Err(TransferError::NotFoundCurrency);
    }

    let mut totals: BTreeMap<(i64, i32), i64> = BTreeMap::new();
    for (unit, receiver_id, amount) in entries {
        *totals.entry((currency_of[unit], *receiver_id)).or_insert(0) += amount;
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
    .fetch_all(&mut *conn)
    .await
    .map_err(TransferError::Database)?;

    let balances: BTreeMap<i64, i64> = locked
        .into_iter()
        .map(|row| (row.currency_id, row.amount.unwrap_or(0)))
        .collect();

    for (currency_id, sent) in &per_currency {
        if balances.get(currency_id).copied().unwrap_or(0) < *sent {
            return Err(TransferError::NotEnoughAmount);
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
    .execute(&mut *conn)
    .await
    .map_err(TransferError::Database)?;

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
    .execute(&mut *conn)
    .await
    .map_err(TransferError::Database)?;

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
    .execute(&mut *conn)
    .await
    .map_err(TransferError::Database)?;

    Ok(())
}
