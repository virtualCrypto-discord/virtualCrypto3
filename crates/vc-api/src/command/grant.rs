//! `Command.handle/4` for `grant`: what a guild decides about applications that
//! want to issue from its pool.
//!
//! No Elixir counterpart. There, a grant only ever appeared as a side effect of
//! redeeming an authorization code, so allowing an application anything meant
//! sending a person to an authorization URL. This command is that decision without
//! the browser: `/grant allow <client_id>` for an application somebody named, and
//! `/grant list` for the ones that asked with an API call.
//!
//! What is mirrored is the shape of the commands beside it: one ephemeral,
//! components-only answer, the administrator bit asked for the way `/issue` asks
//! for it, and a `custom_id` that carries what the screen is about, because an
//! ephemeral message cannot be fetched back.

use serde_json::{Map, Value, json};
use time::OffsetDateTime;

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_OK, CommandError, UPDATE_MESSAGE, as_int,
    as_permissions, is_administrator, value_text,
};
use crate::custom_id::ui::grant::{Action, custom_id, parse};
use crate::error::ApiError;
use crate::state::AppState;

/// How many requests one screen shows. Discord allows five component rows in a
/// message and each request is one row, so a sixth waits for the screen the next
/// decision redraws. A guild with more than five pending requests is a guild whose
/// administrator is looking at this anyway.
const MAX_REQUESTS: usize = 5;

/// `Command.handle/4` for `grant`: the subcommand picks a handler, and both of
/// them need the guild and the administrator bit.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    // Every clause takes a guild: permitting something is an administrator's act,
    // and a direct message has no administrator to be.
    let Some(guild_id) = payload.get("guild_id").and_then(as_int) else {
        return Ok(render_error("エラー: DMでは実行できません。"));
    };

    if !administrator(payload)? {
        return Ok(render_error("エラー: 実行には管理者権限が必要です。"));
    }

    let subcommand = options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("grant has no subcommand"))?;

    match subcommand {
        "allow" => allow(state, options.get("sub_options")).await,
        "list" => page(state, guild_id, CHANNEL_MESSAGE_WITH_SOURCE, None).await,
        _ => Err(CommandError::Unknown),
    }
}

/// `Interaction.Button.handle/4` for the grant screens: a confirmation accepted or
/// dismissed, or one pending request answered.
///
/// The presser is checked exactly as the command is: an ephemeral message is only
/// visible to whoever asked for it, but the button in it outlives their
/// permissions — a person demoted between the two would otherwise still be able to
/// grant.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let Some(guild_id) = payload.get("guild_id").and_then(as_int) else {
        return Ok(render_error("エラー: DMでは実行できません。"));
    };

    if !administrator(payload)? {
        return Ok(render_error("エラー: 実行には管理者権限が必要です。"));
    }

    let (action, subject) = parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    match action {
        Action::Allow => {
            allow_in_guild(state, subject, guild_id).await?;

            Ok(render_update("発行を許可しました。", Some(COLOR_OK)))
        }
        Action::Cancel => Ok(render_update("許可しませんでした。", None)),
        // Both of the list's answers redraw it, so a request that is gone from the
        // list is the whole of what the presser sees of it.
        Action::Approve => {
            let decided = decide(state, subject, guild_id, true).await?;

            let notice = match decided {
                true => "発行を許可しました。",
                false => "エラー: この申請はすでに処理されています。",
            };

            page(state, guild_id, UPDATE_MESSAGE, Some(notice)).await
        }
        Action::Deny => {
            let decided = decide(state, subject, guild_id, false).await?;

            let notice = match decided {
                true => "申請を拒否しました。",
                false => "エラー: この申請はすでに処理されています。",
            };

            page(state, guild_id, UPDATE_MESSAGE, Some(notice)).await
        }
    }
}

/// `/grant allow <client_id>`: what the application is, and the confirmation.
///
/// The guild is not looked at here: what it is about to allow is written when the
/// button in this screen is pressed, and the press is what the guild's permission
/// is checked against.
async fn allow(state: &AppState, sub_options: Option<&Value>) -> Result<Value, CommandError> {
    let client_id = sub_options
        .and_then(|options| options.get("client_id"))
        .map(value_text)
        .ok_or_else(|| CommandError::missing("grant allow has no client_id"))?;

    let Some(application) =
        vc_core::application::find_by_client_id(state.pool(), &client_id).await?
    else {
        return Ok(render_error("エラー: アプリケーションが見つかりません。"));
    };

    let name = name_of(application.client_name.as_deref());

    Ok(json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            Some(COLOR_BRAND as u32),
            vec![
                crate::components::text(format!("**{name}**\n`{client_id}`")),
                crate::components::text("このサーバーのプールからの発行を許可しますか？"),
                crate::components::action_row(vec![
                    crate::components::button(
                        &custom_id(Action::Allow, application.id),
                        "許可する",
                        crate::components::ButtonStyle::Success,
                    ),
                    crate::components::button(
                        &custom_id(Action::Cancel, application.id),
                        "やめる",
                        crate::components::ButtonStyle::Secondary,
                    ),
                ]),
            ],
        )]),
    }))
}

/// `/grant list`, and the screen every decision redraws: what this guild has been
/// asked for and not answered.
async fn page(
    state: &AppState,
    guild_id: i64,
    response_type: i64,
    notice: Option<&str>,
) -> Result<Value, CommandError> {
    let requests = vc_core::grant::requests_in_guild(state.pool(), guild_id).await?;

    let mut children = Vec::new();

    if let Some(notice) = notice {
        children.push(crate::components::text(notice));
    }

    if requests.is_empty() {
        children.push(crate::components::text(
            "発行を申請しているアプリケーションはありません。",
        ));
    } else {
        children.push(crate::components::text(format!(
            "**発行の申請** ({}件)",
            requests.len()
        )));

        for request in requests.iter().take(MAX_REQUESTS) {
            children.push(crate::components::text(format!(
                "**{}**\n`{}`",
                name_of(request.client_name.as_deref()),
                request.client_id,
            )));
            children.push(crate::components::action_row(vec![
                crate::components::button(
                    &custom_id(Action::Approve, request.id),
                    "許可する",
                    crate::components::ButtonStyle::Success,
                ),
                crate::components::button(
                    &custom_id(Action::Deny, request.id),
                    "拒否する",
                    crate::components::ButtonStyle::Danger,
                ),
            ]));
        }

        if requests.len() > MAX_REQUESTS {
            children.push(crate::components::text(format!(
                "ほか{}件。決定すると一覧が進みます。",
                requests.len() - MAX_REQUESTS
            )));
        }
    }

    Ok(json!({
        "type": response_type,
        "data": crate::components::ephemeral(vec![crate::components::container(
            Some(COLOR_BRAND as u32),
            children,
        )]),
    }))
}

/// The guild saying yes, by the two ways in: the confirmation's button and the
/// list's.
async fn allow_in_guild(
    state: &AppState,
    application_id: i64,
    guild_id: i64,
) -> Result<(), CommandError> {
    vc_core::grant::allow_in_guild(
        state.pool(),
        application_id,
        guild_id,
        &[vc_core::application::ISSUE],
        OffsetDateTime::now_utc(),
    )
    .await?;

    Ok(())
}

/// The guild's answer to a request, which writes the grant when it is a yes.
/// `false` is a request that is not this guild's, not there, or already answered.
async fn decide(
    state: &AppState,
    request_id: i64,
    guild_id: i64,
    approved: bool,
) -> Result<bool, CommandError> {
    let scopes: &[&str] = if approved {
        &[vc_core::application::ISSUE]
    } else {
        &[]
    };

    let decided = vc_core::grant::decide_request(
        state.pool(),
        request_id,
        guild_id,
        approved,
        scopes,
        OffsetDateTime::now_utc(),
    )
    .await?;

    Ok(decided.is_some())
}

/// `Command.continue_management_command?/2`'s question, as the interaction
/// carries it: Discord computes the permissions, so there is nothing to look up.
fn administrator(payload: &Value) -> Result<bool, CommandError> {
    let permissions = payload
        .get("member")
        .and_then(|member| member.get("permissions"))
        .and_then(as_permissions)
        .ok_or_else(|| CommandError::missing("grant has no permissions"))?;

    Ok(is_administrator(permissions))
}

fn name_of(client_name: Option<&str>) -> String {
    client_name
        .filter(|name| !name.is_empty())
        .unwrap_or("(名前なし)")
        .to_string()
}

/// An ephemeral answer that replaces the screen it came from: a decision, whose
/// own sentence is all there is left to say.
fn render_update(content: &str, color: Option<i64>) -> Value {
    json!({
        "type": UPDATE_MESSAGE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            color.map(|color| color as u32),
            vec![crate::components::text(content)],
        )]),
    })
}

/// An ephemeral answer with nothing to act on, which is what every refusal here
/// is.
fn render_error(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            None,
            vec![crate::components::text(content)],
        )]),
    })
}
