use serde_json::{Map, Value, json};
use vc_core::currency::DeleteCheck;

use super::{
    ACTION_ROW, CHANNEL_MESSAGE_WITH_SOURCE, CommandError, EPHEMERAL, MODAL, TEXT_INPUT,
    TEXT_INPUT_STYLE_SHORT, as_int,
};
use crate::state::AppState;

/// `Command.handle/4` for `delete`, rendered by `InteractionsJSON.delete/1`
/// through `Interactions.Delete.render/3`.
///
/// The command itself only asks whether the currency could be deleted; the
/// actual deletion happens when the modal it answers with is submitted.
pub async fn handle(
    state: &AppState,
    _options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    // The only `handle/4` clause takes a guild, so Elixir raises in a direct
    // message rather than answering.
    let guild_id = payload
        .get("guild_id")
        .and_then(as_int)
        .ok_or_else(|| CommandError::missing("delete requires a guild"))?;

    let check =
        vc_core::currency::deletable(state.pool(), guild_id, vc_core::model::utc_now()).await?;

    match check {
        DeleteCheck::Deletable { unit } => Ok(render_confirm(&unit)),
        DeleteCheck::NotExist => Ok(render_error("エラー: このサーバーに通貨が存在しません。")),
        DeleteCheck::OutOfTerm => Ok(render_error(
            "エラー: 作成から72時間以上経過しているため削除できません。",
        )),
    }
}

/// `Interactions.Delete.render/3` for `:confirm`: the modal that asks for the
/// currency's unit typed back, whose `custom_id` is what the submission carries.
fn render_confirm(unit: &str) -> Value {
    let required = format!("delete {unit}");

    json!({
        "type": MODAL,
        "data": {
            "title": "通貨の削除",
            "custom_id": crate::custom_id::encode(
                0,
                &crate::custom_id::ui::modal::confirm_currency_delete(),
            ),
            "components": [{
                "type": ACTION_ROW,
                "components": [{
                    "type": TEXT_INPUT,
                    "custom_id": "confirm",
                    "style": TEXT_INPUT_STYLE_SHORT,
                    "label": format!("確認のため、「{required}」と入力してください。"),
                    "placeholder": required,
                }],
            }],
        },
    })
}

/// `Interaction.Modal.handle/4` for `[:delete, :confirm]`: the typed unit is the
/// confirmation, and the currency goes once it matches.
pub async fn confirm(state: &AppState, payload: &Value) -> Result<Value, CommandError> {
    let guild_id = payload
        .get("guild_id")
        .and_then(as_int)
        .ok_or_else(|| CommandError::missing("delete requires a guild"))?;

    let value = payload
        .get("data")
        .and_then(|data| data.get("components"))
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("components"))
        .and_then(Value::as_array)
        .and_then(|inputs| inputs.first())
        .and_then(|input| input.get("value"))
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("the delete modal has no value"))?;

    let Some(unit) = vc_core::currency::unit_for_guild(state.pool(), guild_id).await? else {
        return Ok(render_error("エラー: このサーバーに通貨が存在しません。"));
    };

    if value != format!("delete {unit}") {
        return Ok(render_error(
            "エラー: 確認に失敗しました。再度`/delete`コマンドを実行してください。",
        ));
    }

    vc_core::currency::delete(state.pool(), guild_id).await?;

    Ok(json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "content": "通貨を削除しました。",
            "allowed_mentions": { "parse": [] },
        },
    }))
}

/// `Interactions.Delete.render/3` for `{:error, reason, _}`.
fn render_error(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "content": content,
            "allowed_mentions": { "parse": [] },
        },
    })
}
