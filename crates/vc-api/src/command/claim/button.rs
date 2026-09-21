use serde_json::Value;
use vc_core::claim::{PartialClaim, UpdateClaimsError};

use super::list;
use crate::claim_list::ListOptions;
use crate::command::{CommandError, get_user};
use crate::custom_id::ui::button::{Action, Path, parse};
use crate::error::ApiError;
use crate::state::AppState;

/// `Interaction.Button.handle/4` for `[:claim, :action, X]`: a status change for
/// the claims the list had selected, followed by a redraw of the list.
///
/// The result is not the response body — it is posted as a follow-up message, so
/// the ephemeral list can be updated in place and the outcome still be visible.
pub async fn handle(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let (path, data) = parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    let Path::Act(action) = path else {
        return Err(CommandError::Internal(ApiError::Internal(
            "this button is not a claim action".to_string(),
        )));
    };

    let (options, rest) = ListOptions::parse(&data).ok_or_else(|| {
        CommandError::Internal(ApiError::Internal("the button payload is malformed".into()))
    })?;

    let ids = claim_ids(rest);
    let body = patch(state, me, action, &ids).await?;

    let application_id = payload
        .get("application_id")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("interaction has no application id"))?;
    let token = payload
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("interaction has no token"))?;

    state
        .discord()
        .post_webhook_message(application_id, token, &body)
        .await?;

    list::page(state, me, options).await
}

/// `List.Helper`: the count byte, then that many eight-byte ids.
fn claim_ids(data: &[u8]) -> Vec<i64> {
    let Some((&count, rest)) = data.split_first() else {
        return Vec::new();
    };

    let size = usize::from(count) * 8;
    let ids = rest.get(..size).unwrap_or(rest);

    crate::claim_list::destructuring_claim_ids(ids)
}

/// `Interaction.Button.handle_patch/3`: the status change, as the follow-up body.
async fn patch(
    state: &AppState,
    me: i64,
    action: Action,
    ids: &[i64],
) -> Result<Value, CommandError> {
    let status = match action {
        Action::Approve => "approved",
        Action::Deny => "denied",
        Action::Cancel => "canceled",
    };

    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    let partials: Vec<PartialClaim> = ids
        .iter()
        .map(|id| PartialClaim {
            id: *id,
            status: Some(status.to_string()),
            metadata: None,
        })
        .collect();

    let result =
        vc_core::claim::update_claims(state.pool(), state.notifier(), account, &partials).await;

    let content = match result {
        Ok(updated) => {
            let ids = updated
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(",");

            format!("id: {ids} の請求を{}", result_text(action))
        }
        Err(UpdateClaimsError::InvalidCurrentStatus) => {
            "エラー: 処理しようとした請求はすでに処理済みです。".to_string()
        }
        Err(UpdateClaimsError::PermissionDenied | UpdateClaimsError::InvalidOperator) => {
            "エラー: この請求に対してこの操作を行う権限がありません。".to_string()
        }
        Err(UpdateClaimsError::NotEnoughAmount | UpdateClaimsError::NotFoundSenderAsset) => {
            "エラー: お金が足りません。".to_string()
        }
        Err(UpdateClaimsError::NotFound) => {
            "エラー: そのidの請求は見つかりませんでした。".to_string()
        }
        Err(other) => {
            return Err(CommandError::Internal(ApiError::Internal(format!(
                "the claim update failed: {other:?}"
            ))));
        }
    };

    // A follow-up message rather than an interaction response, and the same shape either way:
    // `content` is what the components flag forbids, so the sentence is a Text Display.
    Ok(crate::components::ephemeral(vec![
        crate::components::container(None, vec![crate::components::text(content)]),
    ]))
}

/// `Interaction.Button.action_str/1`.
fn result_text(action: Action) -> &'static str {
    match action {
        Action::Approve => "承諾し、支払いました。",
        Action::Deny => "拒否しました。",
        Action::Cancel => "キャンセルしました。",
    }
}
