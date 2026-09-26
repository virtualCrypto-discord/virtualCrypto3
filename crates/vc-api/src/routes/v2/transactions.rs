use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};

use crate::routes::idempotency;
use crate::routes::limited::{Authorized, Pay};
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
    user: Authorized<Pay>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let operator_id = user.account_id();

    let asked = asked(&body)?;

    // Operation authorization has already succeeded in the extractor. Check
    // currencies before claiming the key: each unit is resolved to the
    // currency it names and checked. A unit no currency has is left to the payment
    // itself to refuse as `not_found_currency`.
    ensure_currency(&state, &user, &asked).await?;

    let context = idempotency::ReplayContext::Payments {
        grant_id: user.grant_id(),
    };
    idempotency::guard(&state, &headers, operator_id, context, |tx| {
        Box::pin(async move { paid(tx, operator_id, asked, user.resources()).await })
    })
    .await
}

/// Refuse a payment that names a currency the grant does not cover, whether the
/// body carried one or a list of them.
async fn ensure_currency(
    state: &AppState,
    user: &Authorized<Pay>,
    asked: &Asked,
) -> Result<(), ApiError> {
    match asked {
        Asked::Single { unit, .. } => {
            crate::resource::ensure_unit(state.pool(), user.resources(), unit).await
        }
        Asked::Bulk(payments) => {
            for payment in payments {
                crate::resource::ensure_unit(state.pool(), user.resources(), &payment.unit).await?;
            }

            Ok(())
        }
    }
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
            receiver_discord_id: crate::discord_id::parse(receiver.as_str().unwrap_or_default())
                .ok_or(ApiError::InvalidRequest(
                    "invalid_format_of_receiver_discord_id",
                ))?,
            // The name is the *list* clause's, which is where the port carried it
            // from, and it points at a field this request does not have: the
            // mistake is the amount's, so it is named after the amount, as the
            // charge and issue endpoints name it. `docs/known-gaps.md` records the
            // difference, since the Elixir's wording for this clause cannot be
            // checked from here.
            amount: parse_number(amount.as_str().unwrap_or_default())
                .ok_or(ApiError::InvalidRequest("invalid_format_of_amount"))?,
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
            .and_then(crate::discord_id::parse)
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

/// The money, once the body has been read, on the transaction the layer owns.
async fn paid(
    tx: &mut sqlx::PgConnection,
    operator_id: i32,
    asked: Asked,
    resources: &[i64],
) -> Result<(StatusCode, Value), ApiError> {
    // The preflight may precede a wait for an idempotency key. Authorize again
    // under currency locks, which keep the core's unit lookups on these IDs.
    let units = match &asked {
        Asked::Single { unit, .. } => vec![unit.clone()],
        Asked::Bulk(payments) => {
            if payments.iter().any(|payment| payment.amount <= 0) {
                return payment_error(PayError::InvalidAmount);
            }
            payments
                .iter()
                .map(|payment| payment.unit.clone())
                .collect()
        }
    };
    if !crate::resource::lock_units_in(tx, resources, &units).await? {
        return payment_error(PayError::NotFoundCurrency);
    }
    let answered = match asked {
        Asked::Single {
            unit,
            receiver_discord_id,
            amount,
        } => vc_core::payment::pay_in(tx, operator_id, receiver_discord_id, &unit, amount)
            .await
            .map(|()| (StatusCode::CREATED, json!({}))),
        Asked::Bulk(payments) => vc_core::payment::pay_bulk_in(tx, operator_id, &payments)
            .await
            .map(|()| (StatusCode::CREATED, json!({}))),
    };

    match answered {
        Ok(answer) => Ok(answer),
        Err(error) => payment_error(error),
    }
}

/// `UserTransactionView.Pure.render_error/1`: three distinct shapes, and note
/// that `not_found_sender_asset` reports `not_enough_amount`.
///
/// The fourth is not a shape: a payment that failed in the database is one whose
/// transaction rolled back, so the caller has an answer to nothing — an `Err` here
/// is what lets the layer give the key back and the caller retry the same
/// operation rather than being told a week later what went wrong once.
fn payment_error(error: PayError) -> Result<(StatusCode, Value), ApiError> {
    match error {
        PayError::NotFoundCurrency => Ok((
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid_request", "error_info": "not_found_currency" }),
        )),
        PayError::NotEnoughAmount | PayError::NotFoundSenderAsset => Ok((
            StatusCode::CONFLICT,
            json!({ "error": "conflict", "error_info": "not_enough_amount" }),
        )),
        PayError::InvalidAmount => Ok((
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid_request", "error_description": "invalid_amount" }),
        )),
        PayError::Database(error) => Err(ApiError::Core(vc_core::Error::Database(error))),
    }
}

fn parse_number(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}
