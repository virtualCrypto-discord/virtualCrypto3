use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use vc_core::payment::PayError;

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_OK, CommandError, as_int, get_user, mention, option_text,
    value_text,
};
use crate::state::AppState;

/// Acknowledge privately before touching balances. Type 4 makes a later follow-up
/// a new message with its own visibility; the first follow-up after type 5 would
/// instead inherit the original response's ephemeral state.
pub async fn respond(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Response, CommandError> {
    let field = |name| {
        payload
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CommandError::missing(&format!("interaction has no {name}")))
    };
    let interaction_id = field("id")?;
    let application_id = field("application_id")?.to_owned();
    let token = field("token")?.to_owned();
    let acknowledgement = json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": crate::components::EPHEMERAL,
            "content": "処理中…",
            "allowed_mentions": { "parse": [] },
        },
    });
    tokio::time::timeout(
        Duration::from_secs(2),
        state
            .discord()
            .create_interaction_response(interaction_id, &token, &acknowledgement),
    )
    .await
    .map_err(|_| CommandError::missing("the payment acknowledgement timed out"))??;

    let state = state.clone();
    let options = options.clone();
    let payload = payload.clone();
    tokio::spawn(async move {
        let response = match handle(&state, &options, &payload).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(?error, "Discord payment failed");
                render_error("送金結果を確認できませんでした。送金履歴を確認してください。")
            }
        };
        let mut body = response["data"].clone();
        let public = body["flags"].as_u64().unwrap_or_default() & crate::components::EPHEMERAL == 0;
        if public {
            match tokio::time::timeout(
                Duration::from_secs(10),
                state
                    .discord()
                    .post_webhook_message(&application_id, &token, &body),
            )
            .await
            {
                Ok(Ok(())) => {
                    // Only remove the private acknowledgement once the public
                    // success message exists. Notification failures never retry payment.
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        state
                            .discord()
                            .delete_original_interaction_response(&application_id, &token),
                    )
                    .await
                    {
                        Ok(Ok(())) => return,
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "payment acknowledgement deletion failed")
                        }
                        Err(_) => tracing::warn!("payment acknowledgement deletion timed out"),
                    }
                }
                Ok(Err(error)) => tracing::warn!(%error, "payment public response failed"),
                Err(_) => tracing::warn!("payment public response timed out"),
            }
            // Preserve the known success privately if publishing or cleaning up
            // failed, rather than leaving the sender looking at "processing".
        }
        // The original message is already ephemeral. Clear its text to enable
        // components, without trying to change the original message's visibility.
        body["flags"] = json!(crate::components::IS_COMPONENTS_V2);
        body["content"] = Value::Null;
        body["embeds"] = json!([]);
        match tokio::time::timeout(
            Duration::from_secs(10),
            state
                .discord()
                .edit_original_interaction_response(&application_id, &token, &body),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "payment private response failed"),
            Err(_) => tracing::warn!("payment private response timed out"),
        }
    });

    Ok(StatusCode::ACCEPTED.into_response())
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

    let result = vc_core::payment::pay_from_discord(
        state.pool(),
        sender,
        receiver_discord_id,
        &unit,
        amount_value,
    )
    .await;

    match result {
        Ok(()) => Ok(render_ok(sender, &receiver, amount, &unit)),
        Err(PayError::Database(error)) => Err(CommandError::from(error)),
        Err(PayError::NotFoundCurrency) => Ok(render_error("エラー: 通貨は存在しません。")),
        Err(PayError::InvalidAmount) => Ok(render_error("エラー: 不正な金額です。")),
        Err(PayError::NotFoundSenderAsset | PayError::NotEnoughAmount) => {
            Ok(render_error("エラー: 通貨が不足しています。"))
        }
    }
}

/// `Interactions.Pay.render/2` for `:ok`.
///
/// Public, as it was: a payment is said in the channel it happened in. What changed is the shape
/// — an embed is content and a message with components has none — so the sentence is a Text
/// Display in a container with the accent the embed carried. `allowed_mentions` stays, because a
/// mention in a Text Display pings exactly as one in content does.
fn render_ok(sender: i64, receiver: &str, amount: &Value, unit: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": crate::components::IS_COMPONENTS_V2,
            "components": [crate::components::container(
                // The command's colours are `i64` and a container's accent is the 24 bits.
                Some(COLOR_OK as u32),
                vec![crate::components::text(format!(
                    "{}から{}へ**{}** `{}`送金されました。",
                    mention(sender),
                    mention(receiver),
                    value_text(amount),
                    unit,
                ))],
            )],
            "allowed_mentions": { "parse": [] },
        },
    })
}

/// `Interactions.Pay.render/2` for `:error`.
///
/// Ephemeral, as it was, and no accent: the message it was is not an embed and carried no colour.
fn render_error(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": crate::components::EPHEMERAL | crate::components::IS_COMPONENTS_V2,
            "components": [crate::components::container(
                None,
                vec![crate::components::text(content)],
            )],
            "allowed_mentions": { "parse": [] },
        },
    })
}
