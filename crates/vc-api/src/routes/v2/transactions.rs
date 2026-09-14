use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};
use vc_auth::AuthUser;

use crate::routes::idempotency::{self, Idempotency};
use crate::routes::limited::Limited;
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

    let claimed =
        match idempotency::claim(&state, &headers, operator_id, user.scopes.vc_pay).await? {
            Idempotency::None => None,
            Idempotency::Claimed(key) => Some(key),
            Idempotency::Answered(response) => return Ok(response),
        };

    let (status, body) = if let Some(object) = body.as_object() {
        single(&state, &user, operator_id, object).await?
    } else if body.is_array() {
        bulk(&state, &user, operator_id, &body).await?
    } else {
        missing_parameter()
    };

    if let Some(key) = claimed {
        idempotency::register(&state, &key, operator_id, status, &body).await?;

        Ok(idempotency::with_idempotency(status, body, "OK"))
    } else {
        Ok(idempotency::with_idempotency(status, body, "Not Requested"))
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

fn parse_number(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}
