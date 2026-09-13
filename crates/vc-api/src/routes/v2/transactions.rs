use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use vc_auth::AuthUser;

use crate::routes::limited::Limited;
use vc_core::idempotency::Slot;
use vc_core::payment::PayError;

use crate::error::ApiError;
use crate::state::AppState;

/// `POST /api/v2/users/@me/transactions`
///
/// Mirrors `UserTransactionController.post/2`. The idempotency plug runs before
/// the controller in Elixir, so it is handled first here: an unusable key or a
/// token without `vc.pay` is rejected before the body is looked at.
pub async fn post(
    State(state): State<AppState>,
    user: Limited,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let operator_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    // Raw bytes, not `&str`: a permitted key may contain `obs-text`.
    let keys: Vec<&[u8]> = headers
        .get_all("idempotency-key")
        .iter()
        .map(|value| value.as_bytes())
        .collect();

    let claimed = match keys.len() {
        // No key: no idempotency layer at all.
        0 => None,
        1 => {
            let key = vc_core::idempotency::extract_key(keys[0])
                .ok_or(ApiError::InvalidRequest("invalid_idempotency_key"))?;

            if !user.scopes.vc_pay {
                return Err(ApiError::InsufficientScope);
            }

            match vc_core::idempotency::get_or_insert(state.pool(), &key, operator_id)
                .await
                .map_err(db)?
            {
                Slot::Existing {
                    http_status: None, ..
                } => {
                    return Ok(with_idempotency(
                        StatusCode::CONFLICT,
                        vc_core::idempotency::processing(),
                        "Duplicate",
                    ));
                }
                Slot::Existing {
                    http_status: Some(status),
                    body,
                } => {
                    return Ok(with_idempotency(
                        StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK),
                        body.unwrap_or(Value::Null),
                        "Duplicate",
                    ));
                }
                Slot::Created => Some(key),
            }
        }
        // More than one header is refused by the plug itself, with no
        // idempotency-status header.
        _ => {
            return Ok(with_idempotency(
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "invalid_request",
                    "error_description": "multiple_idempotency_key_header_is_not_supported",
                }),
                "",
            ));
        }
    };

    let (status, body) = if let Some(object) = body.as_object() {
        single(&state, &user, operator_id, object).await?
    } else if body.is_array() {
        bulk(&state, &user, operator_id, &body).await?
    } else {
        missing_parameter()
    };

    if let Some(key) = claimed {
        vc_core::idempotency::register(
            state.pool(),
            &key,
            operator_id,
            i32::from(status.as_u16()),
            body.clone(),
        )
        .await
        .map_err(db)?;

        Ok(with_idempotency(status, body, "OK"))
    } else {
        Ok(with_idempotency(status, body, "Not Requested"))
    }
}

/// The single-payment clause: the body must be the object with `unit`,
/// `receiver_discord_id` and `amount`, all strings.
async fn single(
    state: &AppState,
    user: &AuthUser,
    operator_id: i32,
    object: &serde_json::Map<String, Value>,
) -> Result<(StatusCode, Value), ApiError> {
    let Some((unit, receiver, amount)) = (match (
        object.get("unit"),
        object.get("receiver_discord_id"),
        object.get("amount"),
    ) {
        (Some(unit), Some(receiver), Some(amount)) => Some((unit, receiver, amount)),
        _ => None,
    }) else {
        return Ok(missing_parameter());
    };

    if !(unit.is_string() && receiver.is_string() && amount.is_string()) {
        return Ok((
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid_request", "error_description": "invalid_type_of_variable" }),
        ));
    }

    let receiver_discord_id = parse_number(receiver.as_str().unwrap_or_default()).ok_or(
        ApiError::InvalidRequest("invalid_format_of_receiver_discord_id"),
    )?;
    let amount = parse_number(amount.as_str().unwrap_or_default())
        .ok_or(ApiError::InvalidRequest("invalid_format_of_convert_amount"))?;

    if !user.scopes.vc_pay {
        return Err(ApiError::InsufficientScope);
    }

    match vc_core::payment::pay(
        state.pool(),
        operator_id,
        receiver_discord_id,
        unit.as_str().unwrap_or_default(),
        amount,
    )
    .await
    {
        Ok(()) => Ok((StatusCode::CREATED, json!({}))),
        Err(error) => Ok(payment_error(error)),
    }
}

/// The bulk clause: the body is an array of the single-payment objects.
///
/// `convert_list/1` reads each entry's amount, then its unit, then its receiver,
/// and reports the first problem with the index it appeared at.
async fn bulk(
    state: &AppState,
    user: &AuthUser,
    operator_id: i32,
    body: &Value,
) -> Result<(StatusCode, Value), ApiError> {
    let list = body.as_array().expect("the caller checked for an array");

    let mut payments = Vec::with_capacity(list.len());
    for (index, element) in list.iter().enumerate() {
        let object = element.as_object().ok_or(ApiError::BulkInvalid {
            tag: "amount",
            index,
        })?;

        let amount = object
            .get("amount")
            .and_then(Value::as_str)
            .and_then(parse_number)
            .ok_or(ApiError::BulkInvalid {
                tag: "amount",
                index,
            })?;
        let unit = object
            .get("unit")
            .and_then(Value::as_str)
            .ok_or(ApiError::BulkInvalid { tag: "unit", index })?;
        let receiver_discord_id = object
            .get("receiver_discord_id")
            .and_then(Value::as_str)
            .and_then(parse_number)
            .ok_or(ApiError::BulkInvalid {
                tag: "receiver_discord_id",
                index,
            })?;

        payments.push(vc_core::payment::BulkPayment {
            unit: unit.to_string(),
            receiver_discord_id,
            amount,
        });
    }

    if !user.scopes.vc_pay {
        return Err(ApiError::InsufficientScope);
    }

    match vc_core::payment::pay_bulk(state.pool(), operator_id, &payments).await {
        Ok(()) => Ok((StatusCode::CREATED, json!({}))),
        Err(error) => Ok(payment_error(error)),
    }
}

fn missing_parameter() -> (StatusCode, Value) {
    (
        StatusCode::BAD_REQUEST,
        json!({ "error": "invalid_request", "error_description": "missing_parameter" }),
    )
}

/// `UserTransactionView.Pure.render_error/1`: three distinct shapes, and note
/// that `not_found_sender_asset` reports `not_enough_amount`.
fn payment_error(error: PayError) -> (StatusCode, Value) {
    match error {
        PayError::NotFoundCurrency => (
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid_request", "error_info": "not_found_currency" }),
        ),
        PayError::NotEnoughAmount | PayError::NotFoundSenderAsset => (
            StatusCode::CONFLICT,
            json!({ "error": "conflict", "error_info": "not_enough_amount" }),
        ),
        PayError::InvalidAmount => (
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid_request", "error_description": "invalid_amount" }),
        ),
        PayError::Database(error) => {
            tracing::error!(%error, "payment failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "errors": { "detail": "Internal Server Error" } }),
            )
        }
    }
}

fn with_idempotency(status: StatusCode, body: Value, idempotency: &str) -> Response {
    let mut response = (status, Json(body)).into_response();

    if !idempotency.is_empty()
        && let Ok(value) = HeaderValue::from_str(idempotency)
    {
        response.headers_mut().insert("idempotency-status", value);
    }

    response
}

fn parse_number(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}

/// The idempotency helpers report `sqlx::Error` directly, so wrap it.
fn db(error: sqlx::Error) -> ApiError {
    ApiError::Core(vc_core::Error::Database(error))
}
