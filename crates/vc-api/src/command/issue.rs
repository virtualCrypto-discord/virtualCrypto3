use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use vc_core::issue::{IssueError, Issued};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_ERROR, COLOR_OK, CommandError, as_int, as_permissions,
    is_administrator, mention, value_text,
};
use crate::state::AppState;

/// Acknowledge privately before issuing currency. Both success and refusal
/// replace this message, keeping the command's existing ephemeral visibility.
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
    .map_err(|_| CommandError::missing("the issuance acknowledgement timed out"))??;

    let state = state.clone();
    let options = options.clone();
    let payload = payload.clone();
    tokio::spawn(async move {
        let response = match handle(&state, &options, &payload).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(?error, "Discord issuance failed");
                render_error("発行結果を確認できませんでした。発行履歴を確認してください。")
            }
        };
        let mut body = response["data"].clone();
        // Privacy was fixed by the initial response. The renderers also clear
        // content/embeds so this edit can switch the plain text to components.
        body["flags"] = json!(crate::components::IS_COMPONENTS_V2);
        match tokio::time::timeout(
            Duration::from_secs(10),
            state
                .discord()
                .edit_original_interaction_response(&application_id, &token, &body),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "issuance private response failed"),
            Err(_) => tracing::warn!("issuance private response timed out"),
        }
    });

    Ok(StatusCode::ACCEPTED.into_response())
}

/// `Command.handle/4` for `give`, rendered by `InteractionsJSON.give/1` through
/// `Interactions.Give.render/2`.
///
/// The Elixir registered the command as `give`; this service registers it as `issue`, the name
/// the query behind it already had. The handler is otherwise the same.
///
/// This issues currency from the guild's pool rather than moving it between
/// users, so it is the one management command that changes the supply.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    // Every `Command.handle/4` clause for `give` takes a guild, so a direct message has no
    // handler at all; the same is true when the receiver is missing.
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
        .ok_or_else(|| CommandError::missing("issue has no permissions"))?;

    if !is_administrator(permissions) {
        return Ok(render_error("エラー: 実行には管理者権限が必要です。"));
    }

    let receiver_discord_id: i64 = receiver
        .parse()
        .map_err(|_| CommandError::missing("issue receiver is not an id"))?;

    // A missing amount is the `:all` the second `handle/4` clause fills in.
    let amount = match options.get("amount") {
        Some(amount) => Some(
            as_int(amount).ok_or_else(|| CommandError::missing("issue amount is not a number"))?,
        ),
        None => None,
    };

    match vc_core::issue::issue(state.pool(), guild_id, receiver_discord_id, amount).await {
        Ok(issued) => Ok(render_ok(&receiver, &issued)),
        Err(IssueError::Database(error)) => Err(CommandError::from(error)),
        Err(IssueError::NotFoundCurrency) => Ok(render_error("エラー: 通貨が存在しません。")),
        Err(IssueError::InvalidAmount) => Ok(render_error("エラー: 不正な金額です。")),
        Err(IssueError::NotEnoughAmount) => Ok(render_error("エラー: 通貨が不足しています。")),
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
            Some(COLOR_ERROR as u32),
            vec![crate::components::text(content)],
        )]),
    })
}
