use serde_json::{Map, Value, json};
use vc_core::issue::{GiveError, Issued};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_OK, CommandError, as_int, as_permissions, is_administrator,
    mention, value_text,
};
use crate::state::AppState;

/// `Command.handle/4` for `give`, rendered by `InteractionsJSON.give/1` through
/// `Interactions.Give.render/2`.
///
/// This issues currency from the guild's pool rather than moving it between
/// users, so it is the one management command that changes the supply.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    // Every `give` clause takes a guild, so a direct message has no handler at
    // all; the same is true when the receiver is missing.
    let Some(guild_id) = payload.get("guild_id").and_then(as_int) else {
        return Ok(render_error("エラー: DMでは実行できません。"));
    };

    let Some(receiver) = options.get("user").map(value_text) else {
        return Ok(render_error("エラー: DMでは実行できません。"));
    };

    let permissions = payload
        .get("member")
        .and_then(|member| member.get("permissions"))
        .and_then(as_permissions)
        .ok_or_else(|| CommandError::missing("give has no permissions"))?;

    if !is_administrator(permissions) {
        return Ok(render_error("エラー: 実行には管理者権限が必要です。"));
    }

    let receiver_discord_id: i64 = receiver
        .parse()
        .map_err(|_| CommandError::missing("give receiver is not an id"))?;

    // A missing amount is the `:all` the second `handle/4` clause fills in.
    let amount = match options.get("amount") {
        Some(amount) => Some(
            as_int(amount).ok_or_else(|| CommandError::missing("give amount is not a number"))?,
        ),
        None => None,
    };

    match vc_core::issue::give(state.pool(), guild_id, receiver_discord_id, amount).await {
        Ok(issued) => Ok(render_ok(&receiver, &issued)),
        Err(GiveError::Database(error)) => Err(CommandError::from(error)),
        Err(GiveError::NotFoundCurrency) => Ok(render_error("エラー: 通貨が存在しません。")),
        Err(GiveError::InvalidAmount) => Ok(render_error("エラー: 不正な金額です。")),
        Err(GiveError::NotEnoughAmount) => Ok(render_error("エラー: 通貨が不足しています。")),
    }
}

/// `Interactions.Give.render/2` for `:ok`.
fn render_ok(receiver: &str, issued: &Issued) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            // The accent the embed carried.
            Some(COLOR_OK as u32),
            vec![crate::components::text(format!(
                "\u{2705} {}へ**{}** `{}`発行されました。\n残りの発行枠: **{}** `{}`",
                mention(receiver),
                issued.amount,
                issued.unit,
                issued.pool_amount,
                issued.unit,
            ))],
        )]),
    })
}

/// `Interactions.Give.render/2` for `:error`.
///
/// It said `tts: false` where `pay` said nothing, which is the same message: false is the default
/// and the components shape has no place to repeat it.
fn render_error(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            None,
            vec![crate::components::text(content)],
        )]),
    })
}
