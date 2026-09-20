//! `payments_idempotency` — `VirtualCryptoWeb.IdempotencyLayer.Payments`.
//!
//! The key is scoped per user, stored as `bytea`, and expires after seven days
//! (truncated to whole seconds, like every other timestamp in the schema).

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::model::utc_now;

/// How long a key is remembered: "at least seven days, possibly longer".
const LIFETIME: time::Duration = time::Duration::days(7);

#[derive(Debug)]
pub enum Slot {
    /// A row already existed (or a concurrent insert won the race), so its stored
    /// response — if it has one yet — is what the client must see.
    Existing {
        http_status: Option<i32>,
        body: Option<Value>,
    },
    /// This request owns the key and registers the response when it finishes.
    Created,
}

async fn find(
    pool: &PgPool,
    key: &[u8],
    user_id: i32,
) -> std::result::Result<Option<Slot>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT http_status, body FROM payments_idempotency
          WHERE idempotency_key = $1 AND user_id = $2",
        key,
        i64::from(user_id)
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| Slot::Existing {
        http_status: row.http_status,
        body: row.body,
    }))
}

/// `get_or_insert_idempotency_entry/2`: reuse the row when there is one, otherwise
/// claim the key.
pub async fn get_or_insert(
    pool: &PgPool,
    key: &[u8],
    user_id: i32,
) -> std::result::Result<Slot, sqlx::Error> {
    if let Some(slot) = find(pool, key, user_id).await? {
        return Ok(slot);
    }

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
    .fetch_optional(pool)
    .await?;

    if inserted.is_some() {
        return Ok(Slot::Created);
    }

    // A concurrent request inserted between the two statements.
    Ok(find(pool, key, user_id).await?.unwrap_or(Slot::Existing {
        http_status: None,
        body: None,
    }))
}

/// `register_response/2`: store the finished response so a replay can return it,
/// including when it is an error.
pub async fn register(
    pool: &PgPool,
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
    .execute(pool)
    .await?;

    Ok(())
}

/// Give the key back, because the request that claimed it did not happen.
///
/// A claim is a promise to answer, and a request whose transaction rolled back has
/// nothing to answer with: storing the failure would make the key unretryable for
/// a week — the caller's next attempt at the same operation would be answered with
/// a stale error rather than attempted — when what the key protects against is a
/// second *write*, and there was no first one.
///
/// Only the request that claimed the key may do this, and only once it knows the
/// write did not happen. A failure whose effect is unknown is the other case, and
/// is answered by registering it instead.
pub async fn release(
    pool: &PgPool,
    key: &[u8],
    user_id: i32,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query!(
        "DELETE FROM payments_idempotency WHERE idempotency_key = $1 AND user_id = $2",
        key,
        i64::from(user_id)
    )
    .execute(pool)
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
