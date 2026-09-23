//! The `Idempotency-Key` layer, as the endpoints that write share it.
//!
//! It arrived with the payment endpoint, and the issuing and charging endpoints
//! need the same thing for the same reason: a client that retries a request that
//! already succeeded must not pay, issue or charge twice. One copy rather than
//! three, because the three would drift the first time any was touched — the
//! headers, the stored body and the `Idempotency-Status` values are one behaviour.
//!
//! **The claim, the write and the answer are one commit.** [`guard`] opens the
//! transaction — naming `READ COMMITTED`, because the claim's behaviour depends on
//! it — claims the key in it, runs the write on the same connection and
//! registers the answer before committing — so a key can never be left claimed
//! with nothing recorded under it (the state a retry can do nothing with), and a
//! database failure takes its claim down with it, leaving the key free to be used
//! again. A business refusal rolls back its writes but keeps its cached answer.
//! That is also what makes the three endpoints one shape rather than three
//! that each have to remember the order.

use std::future::Future;
use std::pin::Pin;

use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use sqlx::{Acquire, PgConnection};

use vc_core::idempotency::Slot;

use crate::error::ApiError;
use crate::state::AppState;

/// What the header says to do.
enum Idempotency {
    /// No key: no idempotency layer at all.
    None,
    /// This request owns the key, and the transaction owns the claim.
    Claimed(Vec<u8>),
    /// Answer with this instead of running the request.
    Answered(Response),
}

/// A write, on the transaction the layer owns: the answer it produced, or the
/// failure that rolled it back.
///
/// Boxed because the future borrows the connection for as long as it runs, which
/// is the one place the borrow cannot be spelled without it.
pub type Writing<'a> =
    Pin<Box<dyn Future<Output = Result<(StatusCode, Value), ApiError>> + Send + 'a>>;

/// How long a request waits for another request's claim before giving up.
///
/// Waiting is the *right* answer for the ordinary case — a key used twice at once
/// is a client's retry, and the first request is usually milliseconds from
/// finishing, so the honest thing to hand back is its answer rather than "in
/// flight". Waiting forever is not: the row is held for as long as the first
/// request runs, and a request stuck on a slow query or a lock would hold every
/// retry of itself behind it. A second is far longer than a write here takes and
/// far shorter than a client's patience.
const CLAIM_WAIT: &str = "1s";

/// Run a write under its key: claim the key, run the write, store the answer, and
/// commit all three together.
/// Only a successful HTTP answer commits the write; other answers are cached
/// after rolling the write back to a savepoint. An `Err` frees the key too.
///
/// `allowed` is whether the caller's token may do the thing at all — the payment
/// endpoint's scope for paying, the issuing endpoint's for issuing, the contract
/// endpoint's for charging. A key from a token that may not is refused here.
///
/// What the caller has already settled — who they are, what they are asking for,
/// anything the answer needs — it settles *before* this, because a request that
/// never becomes a write must not spend the key.
///
/// Two requests with one key serialize on the claim: the second waits for the
/// first, and the wait is bounded by [`CLAIM_WAIT`] — past that it is answered
/// `409 processing`, which is what tells a client to come back rather than to sit
/// on a connection.
pub async fn guard<F>(
    state: &AppState,
    headers: &HeaderMap,
    identity: i32,
    allowed: bool,
    write: F,
) -> Result<Response, ApiError>
where
    F: for<'a> FnOnce(&'a mut PgConnection) -> Writing<'a>,
{
    // The level this transaction is reasoned about at, named rather than
    // inherited. The claim's insert is expected to **wait** for whoever holds the
    // key and then read the answer — that is `READ COMMITTED` behaviour, and at
    // `REPEATABLE READ` or above the same insert is refused outright, because the
    // row it conflicts with is newer than this transaction's snapshot. A session
    // or server default set to something stricter would change what a retry is
    // answered, without a line of this changing.
    let mut tx = state
        .pool()
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
        .await
        .map_err(db)?;

    let claimed = match claim_in(&mut tx, headers, identity, allowed).await? {
        Idempotency::None => None,
        Idempotency::Claimed(key) => Some(key),
        Idempotency::Answered(response) => {
            // The key already had an answer: nothing is written, and the read the
            // claim made is all this transaction is for.
            tx.rollback().await.map_err(db)?;

            return Ok(response);
        }
    };

    // A business refusal is still an answer to cache, but must not commit any
    // preparatory writes (for example, creating a payment's receiver). Keep the
    // key outside the savepoint so that rolling back the write preserves it.
    let mut operation = tx.begin().await.map_err(db)?;
    let (status, body) = match write(&mut operation).await {
        Ok((status, body)) => {
            if status.is_success() {
                operation.commit().await.map_err(db)?;
            } else {
                operation.rollback().await.map_err(db)?;
            }
            (status, body)
        }
        Err(failure) => {
            // The claim rolls back with the write: it did not happen, and the key
            // is free for the caller to use again — which is what a caller retrying
            // a database blip needs, rather than a stored error it cannot get past.
            operation.rollback().await.map_err(db)?;
            tx.rollback().await.map_err(db)?;

            return Err(failure);
        }
    };

    if let Some(key) = claimed.as_ref() {
        vc_core::idempotency::register_in(
            &mut tx,
            key,
            identity,
            i32::from(status.as_u16()),
            body.clone(),
        )
        .await
        .map_err(db)?;
    }

    tx.commit().await.map_err(db)?;

    Ok(with_idempotency(
        status,
        body,
        if claimed.is_some() {
            "OK"
        } else {
            "Not Requested"
        },
    ))
}

/// `VirtualCryptoWeb.IdempotencyLayer.Payments`: claim the key, or answer as the
/// key's earlier request was answered.
///
/// The header is read first and the database second, so a key that cannot be read
/// at all — malformed, or several of them — is refused without touching the row.
async fn claim_in(
    tx: &mut PgConnection,
    headers: &HeaderMap,
    identity: i32,
    allowed: bool,
) -> Result<Idempotency, ApiError> {
    // Raw bytes, not `&str`: a permitted key may contain `obs-text`.
    let keys: Vec<&[u8]> = headers
        .get_all("idempotency-key")
        .iter()
        .map(|value| value.as_bytes())
        .collect();

    match keys.len() {
        0 => Ok(Idempotency::None),
        1 => {
            let key = vc_core::idempotency::extract_key(keys[0])
                .ok_or(ApiError::InvalidRequest("invalid_idempotency_key"))?;

            if !allowed {
                return Err(ApiError::InsufficientScope);
            }

            // The wait for another request's claim is bounded, for this statement
            // only: the insert is what blocks, and everything after it wants the
            // ordinary timeout back — a write waiting on a row lock is a write that
            // should keep waiting.
            //
            // `set_config` rather than `SET LOCAL`, which cannot take a parameter:
            // the timeout is this function's, and a statement built by formatting
            // one is a statement nobody can check.
            sqlx::query("SELECT set_config('lock_timeout', $1, true)")
                .bind(CLAIM_WAIT)
                .execute(&mut *tx)
                .await
                .map_err(db)?;

            let claimed = vc_core::idempotency::claim_in(tx, &key, identity).await;

            // Whether or not the claim succeeded, everything after it wants the
            // ordinary timeout back. The reset is allowed to fail: an aborted
            // transaction has nothing left that could time out.
            let _ = sqlx::query("SELECT set_config('lock_timeout', '0', true)")
                .execute(&mut *tx)
                .await;

            match claimed {
                Ok(Slot::Created) => Ok(Idempotency::Claimed(key)),
                Ok(Slot::Existing {
                    http_status: Some(status),
                    body,
                }) => Ok(Idempotency::Answered(with_idempotency(
                    StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK),
                    body.unwrap_or(Value::Null),
                    "Duplicate",
                ))),
                // A row with nothing under it: a claim from a version of this
                // service that claimed outside the transaction, and nobody is
                // writing for it. Not a write — "come back".
                Ok(Slot::Existing {
                    http_status: None, ..
                }) => Ok(Idempotency::Answered(processing())),
                // The wait ran out, so whoever holds the key is still at it — and
                // the failed insert leaves this transaction holding nothing, so
                // "come back" is all that is left to say.
                Err(error) if waited_too_long(&error) => Ok(Idempotency::Answered(processing())),
                Err(error) => Err(db(error)),
            }
        }
        // More than one header is refused by the plug itself, with no
        // idempotency-status header.
        _ => Ok(Idempotency::Answered(with_idempotency(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "invalid_request",
                "error_description": "multiple_idempotency_key_header_is_not_supported",
            }),
            "",
        ))),
    }
}

/// The answer for a request whose key another one is using: `409` with the body
/// that says to come back, and the headers that say how and that it was read
/// rather than done.
///
/// The body is the Elixir's, and it says *that* to retry without saying when —
/// `should_retry_after_in_seconds` with no number under it. The number goes in
/// HTTP's own [`header::RETRY_AFTER`], which is what the payments APIs this
/// convention comes from send beside the same kind of answer, and what a retrying
/// client already reads. One second, the wait that just ran out.
///
/// This is also the answer to a claim left by a version of this service that
/// claimed outside its transaction: those rows have no answer coming, and the purge
/// takes them within the seven days a key lives. Telling that caller to come back
/// is the same advice, and it is true — a retry of it is cheap and answers as this
/// one did.
fn processing() -> Response {
    let mut response = with_idempotency(
        StatusCode::CONFLICT,
        vc_core::idempotency::processing(),
        "Duplicate",
    );

    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));

    response
}

/// Whether the claim gave up waiting for another transaction to let go of the row.
///
/// PostgreSQL reports it as `lock_not_available` (`55P03`), which is what the
/// `lock_timeout` around the claim produces — and this is the only way a claim
/// loses here, because the transaction named the isolation level it is reasoned
/// about at. At `REPEATABLE READ` and above the same insert is refused with a
/// serialization failure instead, which is why that level is not left to a
/// session default.
fn waited_too_long(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .as_deref()
        == Some("55P03")
}

/// The response, carrying what the layer did with the key.
pub fn with_idempotency(status: StatusCode, body: Value, idempotency: &str) -> Response {
    let mut response = (status, Json(body)).into_response();

    if !idempotency.is_empty()
        && let Ok(value) = HeaderValue::from_str(idempotency)
    {
        response.headers_mut().insert("idempotency-status", value);
    }

    response
}

/// The idempotency helpers report `sqlx::Error` directly, so wrap it.
fn db(error: sqlx::Error) -> ApiError {
    ApiError::Core(vc_core::Error::Database(error))
}
