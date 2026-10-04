use serde_json::{Map, Value, json};
use vc_core::currency::DeleteCheck;

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_ERROR, CommandError, as_int, as_permissions,
    is_administrator,
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

    if !may_delete(payload) {
        return Ok(render_error(message!("command.delete.handle.001")));
    }

    let check =
        vc_core::currency::deletable(state.pool(), guild_id, vc_core::model::utc_now()).await?;

    match check {
        DeleteCheck::Deletable { unit } => Ok(render_confirm(&unit)),
        DeleteCheck::NotExist => Ok(render_error(message!("command.delete.handle.002"))),
        DeleteCheck::OutOfTerm => Ok(render_error(message!("command.delete.handle.003"))),
    }
}

/// `Interactions.Delete.render/3` for `:confirm`: the modal that asks for the
/// currency's unit typed back, whose `custom_id` is what the submission carries.
fn render_confirm(unit: &str) -> Value {
    let required = format!("delete {unit}");

    // A label rather than an action row: an input inside a row is the form Discord deprecated in
    // modals, and the row existed to carry the `label` field that a label now carries itself.
    crate::components::modal(
        &crate::custom_id::encode(0, &crate::custom_id::ui::modal::confirm_currency_delete()),
        message!("command.delete.render_confirm.001"),
        vec![crate::components::label(
            &format!(
                message!("command.delete.render_confirm.002"),
                required = required
            ),
            None,
            crate::components::text_input(
                "confirm",
                crate::components::TextInputStyle::Short,
                true,
                None,
                None,
                Some(&required),
            ),
        )],
    )
}

/// `Interaction.Modal.handle/4` for `[:delete, :confirm]`: the typed unit is the
/// confirmation, and the currency goes once it matches.
pub async fn confirm(state: &AppState, payload: &Value) -> Result<Value, CommandError> {
    let guild_id = payload
        .get("guild_id")
        .and_then(as_int)
        .ok_or_else(|| CommandError::missing("delete requires a guild"))?;

    // Opening a modal grants no lasting permission: use the signed submission's current roles.
    if !may_delete(payload) {
        return Ok(render_error(message!("command.delete.confirm.001")));
    }

    let value = payload
        .get("data")
        .and_then(|data| data.get("components"))
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        // A label holds its component under `component`, which is the one thing the deprecated
        // action row spelled differently.
        .and_then(|row| {
            row.get("component")
                .or_else(|| row.get("components")?.as_array()?.first())
        })
        .and_then(|input| input.get("value"))
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("the delete modal has no value"))?;

    use vc_core::currency::DeleteResult;
    match vc_core::currency::delete(state.pool(), guild_id, value).await? {
        DeleteResult::Deleted => {}
        DeleteResult::NotExist => {
            return Ok(render_error(message!("command.delete.confirm.002")));
        }
        DeleteResult::OutOfTerm => {
            return Ok(render_error(message!("command.delete.confirm.003")));
        }
        DeleteResult::ConfirmationFailed => {
            return Ok(render_error(&crate::docs::discord::mentions(
                message!("command.delete.confirm.004"),
                state.command_ids().await,
            )));
        }
    }

    Ok(json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            None,
            vec![crate::components::text(message!("command.delete.confirm.005"))],
        )]),
    }))
}

fn may_delete(payload: &Value) -> bool {
    payload
        .get("member")
        .and_then(|member| member.get("permissions"))
        .and_then(as_permissions)
        .is_some_and(is_administrator)
}

/// `Interactions.Delete.render/3` for `{:error, reason, _}`.
fn render_error(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            Some(COLOR_ERROR as u32),
            vec![crate::components::text(content)],
        )]),
    })
}
