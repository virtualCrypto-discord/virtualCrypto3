//! `Command.handle/4` for `grant`: what a guild decides about applications that
//! want to issue from its pool.
//!
//! No Elixir counterpart. There, a grant only ever appeared as a side effect of
//! redeeming an authorization code, so allowing an application anything meant
//! sending a person to an authorization URL. This command is that decision without
//! the browser: `/grant approve code:<user_code>` answers an ask the application
//! made with an API call, `/grant list` shows the asks still waiting, and
//! `/grant revoke code:<user_code|client_id>` takes a permission back.
//!
//! An approval is the only decision there is — there is no refusal, because an
//! ask that is never approved simply stays pending until it expires. And every
//! approval answers an ask: the code names the application's own request, with
//! the scopes it asked for, so a permission nobody asked for cannot be written.
//!
//! What is mirrored is the shape of the commands beside it: one ephemeral,
//! components-only answer, the administrator bit asked for the way `/issue` asks
//! for it, and no buttons at all — the code is typed rather than pressed, so
//! there is no `custom_id` space to keep.

use serde_json::{Map, Value, json};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_OK, CommandError, as_int, as_permissions,
    is_administrator, value_text,
};
use crate::state::AppState;

/// How many requests one screen shows. Five fits in one message without
/// scrolling it off the screen, and a sixth waits for the screen the next
/// approval redraws. A guild with more than five pending requests is a guild
/// whose administrator is looking at this anyway.
const MAX_REQUESTS: usize = 5;

/// `Command.handle/4` for `grant`: the subcommand picks a handler, and all of
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
        "approve" => approve(state, guild_id, options.get("sub_options")).await,
        "revoke" => revoke(state, guild_id, options.get("sub_options")).await,
        "list" => page(state, guild_id).await,
        _ => Err(CommandError::Unknown),
    }
}

/// `/grant approve code:<user_code>`: the guild's yes to one ask.
///
/// The code names the application's own request — the scopes it asked for, in
/// the guild this command runs in — and the grant is written from exactly
/// those. A code that names nothing pending here is answered the same way
/// whether it never existed, belongs to another guild, or already expired:
/// an unapproved ask is not something this command explains.
async fn approve(
    state: &AppState,
    guild_id: i64,
    sub_options: Option<&Value>,
) -> Result<Value, CommandError> {
    let code = sub_options
        .and_then(|options| options.get("code"))
        .map(value_text)
        .ok_or_else(|| CommandError::missing("grant approve has no code"))?;

    let decided = vc_core::grant::decide_request(
        state.pool(),
        code.trim(),
        guild_id,
        time::OffsetDateTime::now_utc(),
    )
    .await?;

    match decided {
        Some(application_id) => {
            // The application's own account, which is what the ping names:
            // the device asked, the guild answered, and this tells it to poll.
            if let Ok(Some(account)) =
                vc_core::user::application_user_id(state.pool(), application_id).await
            {
                state.notifier().notify_grant_decided(account);
            }

            Ok(render_ok("発行を許可しました。"))
        }
        None => Ok(render_error(
            "エラー: そのコードの申請はこのサーバーにありません。",
        )),
    }
}

/// `/grant revoke code:<user_code|client_id>`: the permission back.
///
/// The code is either a pending ask's `user_code`, which revokes the ask's
/// future, or a granted application's `client_id`, which takes the issuing
/// scope back. Either way the scopes go and the rows stay: an approval lives
/// on as history, and a guild token already issued stops issuing because its
/// scopes are read from the grant.
async fn revoke(
    state: &AppState,
    guild_id: i64,
    sub_options: Option<&Value>,
) -> Result<Value, CommandError> {
    let code = sub_options
        .and_then(|options| options.get("code"))
        .map(value_text)
        .ok_or_else(|| CommandError::missing("grant revoke has no code"))?;

    let revoked = vc_core::grant::revoke_grant(state.pool(), code.trim(), guild_id).await?;

    match revoked {
        true => Ok(render_ok("発行の許可を取り消しました。")),
        false => Ok(render_error(
            "エラー: そのコードの申請も許可もこのサーバーにありません。",
        )),
    }
}

/// `/grant list`: what this guild has been asked for and not answered.
///
/// Read-only on purpose: the approval is a typed code rather than a pressed
/// button, so this screen shows the codes to type and nothing to press.
async fn page(state: &AppState, guild_id: i64) -> Result<Value, CommandError> {
    let requests =
        vc_core::grant::requests_in_guild(state.pool(), guild_id, time::OffsetDateTime::now_utc())
            .await?;

    let mut children = Vec::new();

    if requests.is_empty() {
        children.push(crate::components::text(
            "発行を申請しているアプリケーションはありません。",
        ));
    } else {
        children.push(crate::components::text(format!(
            "**発行の申請** ({}件)\n`/grant approve code:` にコードを入れて承認します。",
            requests.len()
        )));

        for request in requests.iter().take(MAX_REQUESTS) {
            children.push(crate::components::text(format!(
                "**{}**\n`{}`\n要求スコープ: {}",
                name_of(request.client_name.as_deref()),
                request.user_code,
                request.scopes.join(" "),
            )));
        }

        if requests.len() > MAX_REQUESTS {
            children.push(crate::components::text(format!(
                "ほか{}件。決定すると一覧が進みます。",
                requests.len() - MAX_REQUESTS
            )));
        }
    }

    Ok(json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            Some(COLOR_BRAND as u32),
            children,
        )]),
    }))
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

/// An ephemeral answer to a decision, whose own sentence is all there is left
/// to say.
fn render_ok(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            Some(COLOR_OK as u32),
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
