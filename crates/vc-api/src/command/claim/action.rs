use serde_json::{Value, json};
use vc_core::claim::{Transition, TransitionError};

use super::{render_error, sub_option};
use crate::command::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_OK, CommandError, EPHEMERAL, as_int, get_user,
};
use crate::error::ApiError;
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
        Err(TransitionError::NotFound) => Ok(render_error("そのidの請求は見つかりませんでした。")),
        Err(TransitionError::InvalidOperator) => Ok(render_error(
            "この請求に対してこの操作を行う権限がありません。",
        )),
        Err(TransitionError::InvalidStatus) => Ok(render_error(
            "この請求に対してこの操作を行うことは出来ません。",
        )),
        Err(TransitionError::NotEnoughAmount | TransitionError::NotFoundSenderAsset) => {
            Ok(render_error("お金が足りません。"))
        }
        Err(TransitionError::NotFoundCurrency) => {
            Ok(render_error("指定された通貨は存在しません。"))
        }
        Err(TransitionError::InvalidAmount) => Ok(render_error(
            "不正な金額です。1以上9223372036854775807以下である必要があります。",
        )),
        // Unreachable: an empty metadata patch cannot push a claim over the
        // entry limit. Reported the way the metadata endpoints report it.
        Err(TransitionError::MetadataLimit) => Err(CommandError::Internal(ApiError::MetadataLimit)),
        Err(TransitionError::Database(error)) => Err(CommandError::from(error)),
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
        "data": {
            "flags": EPHEMERAL,
            "embeds": [{
                "description": format!("id: {claim_id}の請求を{result}"),
                "color": COLOR_OK,
            }],
        },
    })
}
