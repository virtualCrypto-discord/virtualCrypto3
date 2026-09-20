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
/// Mirrors `UserTransactionController.post/2`, with one thing the port settled
/// differently: **the body is read before the key is claimed.** In Elixir the
/// plug claims first and hands an unread body to the controller, so a request the
/// controller then refused left its key claimed with nothing recorded in it —
/// and a retry of the corrected request was answered "still in flight" forever.
/// A body that cannot become a payment is refused without touching the key here;
/// the key's own refusals are still the layer's to answer first.
pub async fn post(
    State(state): State<AppState>,
    user: Limited,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let operator_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    let asked = asked(&body)?;

    let claimed =
        match idempotency::claim(&state, &headers, operator_id, user.scopes.vc_pay).await? {
            Idempotency::None => None,
            Idempotency::Claimed(key) => Some(key),
            Idempotency::Answered(response) => return Ok(response),
        };

    let answered = paid(&state, &user, operator_id, asked).await;

    idempotency::answer(&state, claimed, operator_id, answered).await
}

/// What the body asked for: one payment, or a list of them.
enum Asked {
    Single {
        unit: String,
        receiver_discord_id: i64,
        amount: i64,
    },
    Bulk(Vec<vc_core::payment::BulkPayment>),
}

/// The body, read the way the controller reads it: an object is one payment, an
/// array is a list of them, and anything else is a missing parameter.
///
/// The refusals are the controller's, in its order, and the index a list entry
/// failed at is part of the answer — what changed is only *when* they happen.
fn asked(body: &Value) -> Result<Asked, ApiError> {
    if let Some(object) = body.as_object() {
        // The single-payment clause: `unit`, `receiver_discord_id` and `amount`,
        // all strings.
        let Some((unit, receiver, amount)) = (match (
            object.get("unit"),
            object.get("receiver_discord_id"),
            object.get("amount"),
        ) {
            (Some(unit), Some(receiver), Some(amount)) => Some((unit, receiver, amount)),
            _ => None,
        }) else {
            return Err(ApiError::InvalidRequest("missing_parameter"));
        };

        if !(unit.is_string() && receiver.is_string() && amount.is_string()) {
            return Err(ApiError::InvalidRequest("invalid_type_of_variable"));
        }

        return Ok(Asked::Single {
            unit: unit.as_str().unwrap_or_default().to_owned(),
            receiver_discord_id: parse_number(receiver.as_str().unwrap_or_default()).ok_or(
                ApiError::InvalidRequest("invalid_format_of_receiver_discord_id"),
            )?,
            amount: parse_number(amount.as_str().unwrap_or_default())
                .ok_or(ApiError::InvalidRequest("invalid_format_of_convert_amount"))?,
        });
    }

    if body.is_array() {
        return bulk_asked(body).map(Asked::Bulk);
    }

    Err(ApiError::InvalidRequest("missing_parameter"))
}

/// `convert_list/1` reads each entry's amount, then its unit, then its receiver,
/// and reports the first problem with the index it appeared at.
fn bulk_asked(body: &Value) -> Result<Vec<vc_core::payment::BulkPayment>, ApiError> {
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

    Ok(payments)
}

/// The money, once the body has been read: the scope the caller needs, and then
/// the payment.
async fn paid(
    state: &AppState,
    user: &AuthUser,
    operator_id: i32,
    asked: Asked,
) -> Result<(StatusCode, Value), ApiError> {
    if !user.scopes.vc_pay {
        return Err(ApiError::InsufficientScope);
    }

    let answered = match asked {
        Asked::Single {
            unit,
            receiver_discord_id,
            amount,
        } => vc_core::payment::pay(
            state.pool(),
            operator_id,
            receiver_discord_id,
            &unit,
            amount,
        )
        .await
        .map(|()| (StatusCode::CREATED, json!({}))),
        Asked::Bulk(payments) => vc_core::payment::pay_bulk(state.pool(), operator_id, &payments)
            .await
            .map(|()| (StatusCode::CREATED, json!({}))),
    };

    Ok(answered.unwrap_or_else(payment_error))
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
