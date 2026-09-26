pub mod action;
pub mod button;
pub mod list;
pub mod make;
pub mod show;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use time::PrimitiveDateTime;
use vc_core::claim::{ClaimUser, Transition, TransitionError};

use crate::claim_list::Position;

use super::{CHANNEL_MESSAGE_WITH_SOURCE, COLOR_ERROR, CommandError};
use crate::error::ApiError;
use crate::state::AppState;

/// The mutating slash commands acknowledge privately before doing any DB work.
pub async fn respond(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Response, CommandError> {
    let reply = super::response::Acknowledged::private(state, payload).await?;
    let state = state.clone();
    let options = options.clone();
    let payload = payload.clone();
    tokio::spawn(async move {
        let response = handle(&state, &options, &payload)
            .await
            .unwrap_or_else(failed);
        reply.edit(response["data"].clone()).await;
    });
    Ok(StatusCode::ACCEPTED.into_response())
}

fn failed(error: CommandError) -> Value {
    tracing::warn!(?error, "Discord claim operation failed");
    render_error("請求の処理結果を確認できませんでした。請求の状態と履歴を確認してください。")
}

/// A Discord user or Bot is mentioned; an unbound application needs its public identity.
fn user_identity(user: &ClaimUser) -> String {
    if let Some(discord_id) = user.discord_id {
        super::mention(discord_id)
    } else if let Some(client_id) = user.client_id.as_deref() {
        super::application_identity(None, client_id, user.client_name.as_deref())
    } else {
        format!("不明なアカウント（ID: {}）", user.id)
    }
}

/// `Command.handle/4`'s `"claim"` clauses: the subcommand picks a handler.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let subcommand = options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("claim has no subcommand"))?;

    let sub_options = options.get("sub_options");

    match subcommand {
        "approve" => action::handle(state, sub_options, payload, Transition::Approved).await,
        "cancel" => action::handle(state, sub_options, payload, Transition::Canceled).await,
        "deny" => action::handle(state, sub_options, payload, Transition::Denied).await,
        "make" => make::handle(state, sub_options, payload).await,
        "list" => list::handle(state, sub_options, payload, Position::All).await,
        "received" => list::handle(state, sub_options, payload, Position::Received).await,
        "sent" => list::handle(state, sub_options, payload, Position::Claimed).await,
        "show" => show::handle(state, sub_options, payload).await,
        _ => Err(CommandError::Unknown),
    }
}

/// An option inside `sub_options`. Elixir indexes the nested map directly, so a
/// missing one is a raise rather than an answer.
fn sub_option<'a>(sub_options: Option<&'a Value>, name: &str) -> Result<&'a Value, CommandError> {
    sub_options
        .and_then(|options| options.get(name))
        .ok_or_else(|| CommandError::missing(&format!("claim option {name}")))
}

/// `Interactions.Util.format_date_time/1`: Discord renders `<t:unix>` in the
/// reader's own timezone.
///
/// Shared with the contract screen, which has a deadline to say the same way and
/// would otherwise have a second copy of this format.
pub(super) fn format_date_time(value: PrimitiveDateTime) -> String {
    format!("<t:{}>", value.assume_utc().unix_timestamp())
}

/// `Claim.render_error/1`: the sentence each refusal of a claim's transition gets.
///
/// One function because the Elixir has one: the `claim approve|deny|cancel` command and
/// the buttons that move a claim both answer a refusal with it.
pub(super) fn transition_error(error: TransitionError) -> Result<Value, CommandError> {
    match error {
        TransitionError::NotFound => Ok(render_error("そのidの請求は見つかりませんでした。")),
        TransitionError::InvalidOperator => Ok(render_error(
            "この請求に対してこの操作を行う権限がありません。",
        )),
        TransitionError::InvalidStatus => Ok(render_error(
            "この請求に対してこの操作を行うことは出来ません。",
        )),
        TransitionError::NotEnoughAmount | TransitionError::NotFoundSenderAsset => {
            Ok(render_error("お金が足りません。"))
        }
        TransitionError::NotFoundCurrency => Ok(render_error("指定された通貨は存在しません。")),
        TransitionError::InvalidAmount => Ok(render_error(
            "不正な金額です。1以上9223372036854775807以下である必要があります。",
        )),
        // Unreachable here: both callers move a claim without a metadata patch of their
        // own, which is the only way a claim's metadata grows. Reported the way the
        // metadata endpoints report it.
        TransitionError::MetadataLimit => Err(CommandError::Internal(ApiError::MetadataLimit)),
        TransitionError::Database(error) => Err(CommandError::from(error)),
    }
}

/// `Interactions.Claim.render/1` for `{:error, _, error}`.
///
/// The title is a line of its own, as an embed's title was: components have no title, so it is
/// bold text above the sentence it introduced.
fn render_error(description: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            Some(COLOR_ERROR as u32),
            vec![crate::components::text(format!("**エラー**\n{description}"))],
        )]),
    })
}
