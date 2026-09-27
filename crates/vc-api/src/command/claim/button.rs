use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::Value;
use vc_core::claim::{PartialClaim, UpdateClaimsError};

use super::{list, show};
use crate::claim_list::ListOptions;
use crate::command::{COLOR_ERROR, COLOR_OK, CommandError, get_user};
use crate::custom_id::ui::button::{Action, Path, parse};
use crate::error::ApiError;
use crate::state::AppState;

/// `Interaction.Button.handle/4` for a claim's buttons, which are three kinds:
/// a list row's action, the list's own pagination row, and one claim's screen.
///
/// A row's action is a status change for the claims it names, followed by a redraw of
/// the list; the redraw is the response and the outcome is posted as a follow-up, so the
/// ephemeral list updates in place and the result is still visible. The other two answer
/// with the screen they came from, updated: the Elixir's three `handle/4` clauses.
pub async fn handle(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Response, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let (path, data) = parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    let action = match path {
        // `[:claim, :list, position]`: the pagination row — the four arrows and the
        // reload. It only redraws, and the page to draw is in the options the button
        // carries rather than in the path, which is why the scope is not read: the
        // Elixir's `handle_listing/2` reads the options too.
        Path::List(_) => {
            return Ok(Json(list::page(state, me, options(&data)?).await?).into_response());
        }
        // `[:claim, :action_single, action]`: one claim's own screen, which answers with
        // itself rather than with a list.
        Path::ActionSingle(action) => {
            let id = single_claim_id(&data)?;
            let reply = crate::command::response::Acknowledged::update(state, payload).await?;
            let state = state.clone();
            tokio::spawn(async move {
                let response = show::pressed(&state, me, action, id)
                    .await
                    .unwrap_or_else(super::failed);
                let body = response["data"].clone();
                if response["type"] == crate::command::UPDATE_MESSAGE
                    || reply.followup(&body).await.is_none()
                {
                    reply.edit(body).await;
                }
            });
            return Ok(StatusCode::ACCEPTED.into_response());
        }
        Path::Act(action) => action,
    };

    let (options, rest) = ListOptions::parse(&data).ok_or_else(|| {
        CommandError::Internal(ApiError::Internal("the button payload is malformed".into()))
    })?;

    let ids = claim_ids(rest);
    let reply = crate::command::response::Acknowledged::update(state, payload).await?;
    let state = state.clone();
    tokio::spawn(async move {
        let body = patch(&state, me, action, &ids)
            .await
            .unwrap_or_else(|error| super::failed(error)["data"].clone());
        // A failed redraw must not discard the already-known operation result.
        match list::page(&state, me, options).await {
            Ok(page) => {
                reply.edit(page["data"].clone()).await;
            }
            Err(error) => tracing::warn!(?error, "claim list redraw failed"),
        }
        if reply.followup(&body).await.is_none() {
            reply.edit(body).await;
        }
    });

    // The callback already supplied the interaction response. Discord requires
    // an empty 202 on the incoming HTTP request in this case.
    Ok(StatusCode::ACCEPTED.into_response())
}

/// The options a list button carries, which is the whole state of the page it draws.
fn options(data: &[u8]) -> Result<ListOptions, CommandError> {
    ListOptions::parse(data)
        .map(|(options, _)| options)
        .ok_or_else(|| {
            CommandError::Internal(ApiError::Internal("the button payload is malformed".into()))
        })
}

/// `Show.action_custom_id/3`'s tail: the claim's id as eight big-endian bytes.
fn single_claim_id(data: &[u8]) -> Result<i64, CommandError> {
    let id: [u8; 8] = data
        .get(..8)
        .and_then(|head| head.try_into().ok())
        .ok_or_else(|| {
            CommandError::Internal(ApiError::Internal("the button payload is malformed".into()))
        })?;

    Ok(i64::from_be_bytes(id))
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

    let color = if result.is_err() {
        COLOR_ERROR
    } else {
        COLOR_OK
    };
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
        crate::components::container(Some(color as u32), vec![crate::components::text(content)]),
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
