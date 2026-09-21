//! `payments_idempotency` — `VirtualCryptoWeb.IdempotencyLayer.Payments`.
//!
//! The key is scoped per user, stored as `bytea`, and expires after seven days
//! (truncated to whole seconds, like every other timestamp in the schema).
//!
//! **The claim lives in the caller's transaction, and that is the design.** The
//! row that says "this key is in use" is inserted by the same transaction that
//! performs the write and stores the answer, so claim, write and answer are one
//! commit. A process that dies mid-request takes the claim down with it, and a key
//! can never be left claimed with nothing recorded under it — the state a retry
//! can do nothing with.
//!
//! Two concurrent requests with one key serialize on the unique index: the second
//! blocks inside its `INSERT` until the first commits, and then reads the stored
//! answer, or until the first rolls back, and then takes the key itself. Measured,
//! not assumed: at `READ COMMITTED` — what this service runs — that is exactly
//! what happens, and the answer the second one reads is the first one's. Under
//! `REPEATABLE READ` or `SERIALIZABLE` the blocked insert is instead answered with
//! a serialization failure, which aborts its transaction: nothing is written, no
//! claim survives, and the caller may retry with the same key.
//!
//! **A key is not compared against the request that carries it.** A row that
//! already answers is answered from the row, whatever body arrives under it: the
//! specification defines no way to tell whether two requests are the same, so a
//! comparison written here would be this module's invention — and one that refuses
//! honest retries (a bulk list in another order, a field this API ignores) as
//! readily as it caught a caller reusing a key for another charge. The caller's key
//! is its own word for one request; `docs/known-gaps.md` carries the decision and
//! what it costs.

use serde_json::{Value, json};
use sqlx::PgConnection;

use crate::model::utc_now;

/// How long a key is remembered: "at least seven days, possibly longer".
const LIFETIME: time::Duration = time::Duration::days(7);

#[derive(Debug)]
pub enum Slot {
    /// A row was already there, so its stored response — if it has one yet — is
    /// what the client must see. `http_status` is `None` for a row whose claim is
    /// still being held by another transaction (which cannot happen at the
    /// isolation level this service runs at, because the insert would have waited
    /// for it) or for one left by a version of this service that claimed outside
    /// the transaction.
    Existing {
        http_status: Option<i32>,
        body: Option<Value>,
    },
    /// This request owns the key, and the transaction it is in owns the claim.
    Created,
}

async fn find(
    tx: &mut PgConnection,
    key: &[u8],
    user_id: i32,
) -> std::result::Result<Option<Slot>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT http_status, body FROM payments_idempotency
          WHERE idempotency_key = $1 AND user_id = $2",
        key,
        i64::from(user_id)
    )
    .fetch_optional(&mut *tx)
    .await?;

    Ok(row.map(|row| Slot::Existing {
        http_status: row.http_status,
        body: row.body,
    }))
}

/// `get_or_insert_idempotency_entry/2`, inside the caller's transaction: reuse the
/// row when there is one, otherwise claim the key — and if another transaction is
/// holding it, wait there rather than here.
pub async fn claim_in(
    tx: &mut PgConnection,
    key: &[u8],
    user_id: i32,
) -> std::result::Result<Slot, sqlx::Error> {
    let now = utc_now();

    let inserted = sqlx::query_scalar!(
        "INSERT INTO payments_idempotency
             (user_id, idempotency_key, expires, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)
         ON CONFLICT (idempotency_key, user_id) DO NOTHING
         RETURNING id",
        i64::from(user_id),
        key,
        now + LIFETIME,
        now
    )
    .fetch_optional(&mut *tx)
    .await?;

    if inserted.is_some() {
        return Ok(Slot::Created);
    }

    // The insert did nothing, so a row is there — and by the time this statement
    // runs, whatever transaction was holding it has committed or gone.
    Ok(find(&mut *tx, key, user_id)
        .await?
        .unwrap_or(Slot::Existing {
            http_status: None,
            body: None,
        }))
}

/// `register_response/2`: store the finished response so a replay can return it,
/// including when it is an error — in the same transaction as the write, so the
/// two cannot come apart.
pub async fn register_in(
    tx: &mut PgConnection,
    key: &[u8],
    user_id: i32,
    http_status: i32,
    body: Value,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE payments_idempotency
            SET body = $3, http_status = $4, updated_at = $5
          WHERE idempotency_key = $1 AND user_id = $2",
        key,
        i64::from(user_id),
        body,
        http_status,
        utc_now()
    )
    .execute(&mut *tx)
    .await?;

    Ok(())
}

/// The replay body for a request that is still in flight.
pub fn processing() -> Value {
    json!({
        "error": "processing",
        "error_description": "should_retry_after_in_seconds",
    })
}

/// `Validator.extract_idempotency_key/1`: the value must be a double-quoted
/// string of 0–256 characters, where a character is `!`, `#`–`~` or `0x80`–`0xFF`.
///
/// Takes raw bytes rather than `&str` on purpose: a permitted key can contain
/// `obs-text`, which is not valid UTF-8 and would be lost by `HeaderValue::to_str`.
pub fn extract_key(header: &[u8]) -> Option<Vec<u8>> {
    if header.len() < 2 || header[0] != b'"' || header[header.len() - 1] != b'"' {
        return None;
    }

    let inner = &header[1..header.len() - 1];
    if inner.len() > 256 {
        return None;
    }

    inner
        .iter()
        .all(|byte| *byte == b'!' || (b'#'..=b'~').contains(byte) || *byte >= 0x80)
        .then(|| inner.to_vec())
}
