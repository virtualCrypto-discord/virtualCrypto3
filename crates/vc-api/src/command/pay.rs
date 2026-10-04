use std::time::Duration;

use axum::Json;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use tokio::time::{Instant, timeout_at};
use vc_core::payment::PayError;

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_ERROR, COLOR_OK, CommandError, as_int, error_text, get_user,
    mention, money_text, option_text,
};
use crate::state::AppState;

/// Decide visibility only after the outcome is known. The initial response is
/// the result itself: no processing message, deferred reply or follow-up.
pub async fn respond(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
    received_at: Instant,
) -> Result<Response, CommandError> {
    let response = handle_until(state, options, payload, received_at)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(?error, "Discord payment failed");
            uncertain_result()
        });
    Ok(Json(response).into_response())
}

pub(crate) fn busy_response() -> Value {
    render_error(message!("command.pay.busy_response.001"))
}

pub(crate) fn uncertain_result() -> Value {
    render_error(message!("command.pay.uncertain_result.001"))
}

/// `Command.handle/4` for `pay`, rendered by `InteractionsJSON.pay/1` through
/// `Interactions.Pay.render/2`.
///
/// Both sides are discord users here: `Money.pay/1` is given a `DiscordUser` for
/// the sender and the receiver, so either may have no account yet.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    handle_until(state, options, payload, Instant::now()).await
}

async fn handle_until(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
    received_at: Instant,
) -> Result<Value, CommandError> {
    let sender =
        get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let unit = option_text(options, "unit")?;
    let receiver = option_text(options, "user")?;
    let amount = options
        .get("amount")
        .ok_or_else(|| CommandError::missing("pay has no amount"))?;

    let receiver_discord_id: i64 = receiver
        .parse()
        .map_err(|_| CommandError::missing("pay receiver is not an id"))?;
    let amount_value =
        as_int(amount).ok_or_else(|| CommandError::missing("pay amount is not a number"))?;

    // Cancel only preparation when it takes too long: dropping its uncommitted
    // transaction rolls back accounts, balances and history together. Include
    // time spent acquiring the interaction receipt in this budget.
    let prepared = timeout_at(received_at + Duration::from_secs(1), async {
        let mut tx = state.pool().begin().await.map_err(PayError::Database)?;
        vc_core::payment::pay_from_discord_in(
            &mut tx,
            sender,
            receiver_discord_id,
            &unit,
            amount_value,
        )
        .await?;
        Ok::<_, PayError>(tx)
    })
    .await;

    let tx = match prepared {
        Err(_) => return Ok(busy_response()),
        Ok(Ok(tx)) => tx,
        Ok(Err(PayError::Database(error))) => {
            tracing::warn!(%error, "Discord payment preparation failed");
            return Ok(render_error(message!("command.pay.handle_until.001")));
        }
        Ok(Err(PayError::NotFoundCurrency)) => {
            return Ok(render_error(message!("command.pay.handle_until.002")));
        }
        Ok(Err(PayError::InvalidAmount)) => {
            return Ok(render_error(message!("command.pay.handle_until.003")));
        }
        Ok(Err(PayError::NotFoundSenderAsset | PayError::NotEnoughAmount)) => {
            return Ok(render_error(message!("command.pay.handle_until.004")));
        }
    };

    let commit_deadline = received_at + Duration::from_secs(2);
    if Instant::now() >= commit_deadline {
        return Ok(busy_response()); // No COMMIT has been sent.
    }
    // Once COMMIT starts, a timeout or lost connection has an uncertain outcome.
    // Never label it as unpaid or retry it. The durable interaction receipt
    // prevents a replay from executing the payment again.
    match timeout_at(commit_deadline, tx.commit()).await {
        Ok(Ok(())) => Ok(render_ok(sender, &receiver, amount_value, &unit)),
        Ok(Err(error)) => {
            tracing::warn!(%error, "Discord payment commit failed");
            Ok(uncertain_result())
        }
        Err(_) => {
            tracing::warn!("Discord payment commit timed out");
            Ok(uncertain_result())
        }
    }
}

/// `Interactions.Pay.render/2` for `:ok`.
///
/// Public, as it was: a payment is said in the channel it happened in. What changed is the shape
/// — an embed is content and a message with components has none — so the sentence is a Text
/// Display in a container with the accent the embed carried. `allowed_mentions` stays, because a
/// mention in a Text Display pings exactly as one in content does.
fn render_ok(sender: i64, receiver: &str, amount: i64, unit: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": crate::components::IS_COMPONENTS_V2,
            "components": [crate::components::container(
                // The command's colours are `i64` and a container's accent is the 24 bits.
                Some(COLOR_OK as u32),
                vec![crate::components::text(format!(
                    message!("command.pay.render_ok.001"),
                    mention(sender),
                    mention(receiver),
                    money_text(amount, unit),
                ))],
            )],
            "allowed_mentions": { "parse": [] },
        },
    })
}

/// `Interactions.Pay.render/2` for `:error`.
///
/// Private, with the shared error accent for both refusals and uncertain results.
fn render_error(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": crate::components::EPHEMERAL | crate::components::IS_COMPONENTS_V2,
            "components": [crate::components::container(
                Some(COLOR_ERROR as u32),
                vec![crate::components::text(error_text(content))],
            )],
            "allowed_mentions": { "parse": [] },
        },
    })
}
