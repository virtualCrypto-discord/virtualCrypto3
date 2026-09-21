use serde_json::{Value, json};
use vc_core::claim::Transition;

use super::sub_option;
use crate::command::{CHANNEL_MESSAGE_WITH_SOURCE, COLOR_OK, CommandError, as_int, get_user};
use crate::state::AppState;

/// `Command.handle/4` for `claim approve`, `claim deny` and `claim cancel`, which
/// differ only in the status they move the claim to.
pub async fn handle(
    state: &AppState,
    sub_options: Option<&Value>,
    payload: &Value,
    transition: Transition,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let id = as_int(sub_option(sub_options, "id")?)
        .ok_or_else(|| CommandError::missing("claim id is not a number"))?;

    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    // The command carries no metadata of its own; `%{}` is what Elixir passes,
    // which upserts an empty row for the operator.
    let result = vc_core::claim::transition(
        state.pool(),
        state.notifier(),
        account,
        id,
        transition,
        Some(json!({})),
    )
    .await;

    match result {
        Ok(()) => Ok(render(transition, id)),
        Err(error) => super::transition_error(error),
    }
}

/// `Interactions.Claim.render/1` for `{:ok, "approve" | "deny" | "cancel", claim}`.
fn render(transition: Transition, claim_id: i64) -> Value {
    let result = match transition {
        Transition::Approved => "承諾し、支払いました。",
        Transition::Denied => "拒否しました。",
        Transition::Canceled => "キャンセルしました。",
    };

    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            // The accent the embed carried.
            Some(COLOR_OK as u32),
            vec![crate::components::text(format!(
                "id: {claim_id}の請求を{result}"
            ))],
        )]),
    })
}
