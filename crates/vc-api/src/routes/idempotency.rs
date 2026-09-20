//! The `Idempotency-Key` layer, as the endpoints that write share it.
//!
//! It arrived with the payment endpoint, and the issuing endpoint needs the same
//! thing for the same reason: a client that retries a request that already
//! succeeded must not pay or issue twice. One copy rather than two, because the
//! two would drift the first time either was touched — the headers, the stored
//! body and the `Idempotency-Status` values are one behaviour, not two.

use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use vc_core::idempotency::Slot;

use crate::error::ApiError;
use crate::state::AppState;

/// What the header says to do, decided before the body is looked at.
pub enum Idempotency {
    /// No key: no idempotency layer at all.
    None,
    /// This request owns the key and registers its answer when it has one.
    Claimed(Vec<u8>),
    /// Answer with this instead of running the request.
    Answered(Response),
}

/// `VirtualCryptoWeb.IdempotencyLayer.Payments`: claim the key, or answer as the
/// key's earlier request was answered.
///
/// `allowed` is whether the caller's token may do the thing at all — the payment
/// endpoint's scope for paying, the issuing endpoint's for issuing. A key from a
/// token that may not is refused here, before anything is parsed, which is where
/// the plug sits in the Elixir.
pub async fn claim(
    state: &AppState,
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

            match vc_core::idempotency::get_or_insert(state.pool(), &key, identity)
                .await
                .map_err(db)?
            {
                Slot::Existing {
                    http_status: None, ..
                } => Ok(Idempotency::Answered(with_idempotency(
                    StatusCode::CONFLICT,
                    vc_core::idempotency::processing(),
                    "Duplicate",
                ))),
                Slot::Existing {
                    http_status: Some(status),
                    body,
                } => Ok(Idempotency::Answered(with_idempotency(
                    StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK),
                    body.unwrap_or(Value::Null),
                    "Duplicate",
                ))),
                Slot::Created => Ok(Idempotency::Claimed(key)),
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

/// `register_response/2`, once the request has produced something to remember.
pub async fn register(
    state: &AppState,
    key: &[u8],
    identity: i32,
    status: StatusCode,
    body: &Value,
) -> Result<(), ApiError> {
    vc_core::idempotency::register(
        state.pool(),
        key,
        identity,
        i32::from(status.as_u16()),
        body.clone(),
    )
    .await
    .map_err(db)
}

/// The answer to a request that has run, given what it produced.
///
/// Three cases, and the difference between them is what the write did rather than
/// what status it produced:
///
/// - **`Ok`** — the endpoint answered. Success or refusal, it is stored under a
///   claimed key, because a replay has to get that answer rather than a second
///   attempt at a write that already happened.
/// - **`Err`, claimed** — the request failed without happening: the transaction
///   rolled back, nothing moved, and the key is **given back** (`release`). A key
///   that stored the failure would be unretryable for a week — the caller's next
///   attempt at the same operation answered with a stale error instead of
///   attempted — when what the key exists for is to stop a second write, and there
///   was not a first one.
/// - **`Err`, no key** — answered exactly as it would have been without this layer:
///   a failure is the endpoint's to report, and the header is not the layer's to
///   add.
///
/// What this costs is the case it cannot see: a failure whose effect is *unknown*
/// must be registered rather than released, or a retry could write twice. Each
/// handler answers that by making its failures unambiguous — the contract
/// endpoint reads what it needs for the answer before it charges, so the only way
/// it can fail is a rolled-back transaction.
pub async fn answer(
    state: &AppState,
    claimed: Option<Vec<u8>>,
    identity: i32,
    answered: Result<(StatusCode, Value), ApiError>,
) -> Result<Response, ApiError> {
    let (status, body) = match answered {
        Ok(answer) => answer,
        Err(failure) => {
            if let Some(key) = claimed.as_ref() {
                release(state, key, identity).await?;
            }

            return Err(failure);
        }
    };

    match claimed {
        Some(key) => {
            register(state, &key, identity, status, &body).await?;

            Ok(with_idempotency(status, body, "OK"))
        }
        None => Ok(with_idempotency(status, body, "Not Requested")),
    }
}

/// `IdempotencyLayer`'s counterpart to [`claim`]: the key goes back, and the next
/// request that names it is a request again rather than a replay.
pub async fn release(state: &AppState, key: &[u8], identity: i32) -> Result<(), ApiError> {
    vc_core::idempotency::release(state.pool(), key, identity)
        .await
        .map_err(db)
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
