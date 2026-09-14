use serde_json::{Map, Value, json};
use vc_core::payment::PayError;

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_OK, CommandError, as_int, get_user, mention, option_text,
    value_text,
};
use crate::state::AppState;

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
