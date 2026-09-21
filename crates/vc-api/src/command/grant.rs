//! `Command.handle/4` for `grant`: what a guild decides about applications that
//! want to issue from its pool.
//!
//! No Elixir counterpart. There, a grant only ever appeared as a side effect of
//! redeeming an authorization code, so allowing an application anything meant
//! sending a person to an authorization URL. This command is that decision without
//! the browser: `/grant approve code:<user_code>` answers an ask the application
//! made with an API call, and `/grant list` shows the applications this guild has
//! allowed, each with the button that takes the permission back.
//!
//! The approval is typed and the taking-back is pressed, and that is the whole of
//! the division: the code names a row only the application can put in front of a
//! person, while a permission the guild has granted is a row it can see. Asks in
//! general are the application's own business — it holds the `device_code` it
//! polls with, and `GET /oauth2/clients/@me/grant-requests` is where it reads
//! them — so nothing here lists them.
//!
//! An approval is the only decision there is — there is no refusal, because an
//! ask that is never approved simply stays pending until it expires. And every
//! approval answers an ask: the code names the application's own request, with
//! the scopes it asked for, so a permission nobody asked for cannot be written.

use serde_json::{Map, Value, json};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, COLOR_OK, CommandError, UPDATE_MESSAGE,
    as_int, as_permissions, is_administrator, value_text,
};
use crate::components::{ButtonStyle, action_row, button, container, ephemeral, icon_button, text};
use crate::custom_id::ui::grant::{Page, Pressed, page_custom_id, revoke_custom_id};
use crate::error::ApiError;
use crate::state::AppState;

/// How many applications one screen shows, for the contract screen's reason: five
/// fits in one message without scrolling it off the screen, and a sixth waits for
/// the screen the next revoke redraws.
const MAX_APPLICATIONS: usize = 5;

/// Where the screen starts, and where a revoke draws next: page numbers count from
/// one, as the claim list's do.
const FIRST_PAGE: i64 = 1;

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
        "approve" => approve(state, guild_id, options.get("sub_options")).await,
        "list" => Ok(answered(
            page(state, guild_id, FIRST_PAGE).await?,
            CHANNEL_MESSAGE_WITH_SOURCE,
        )),
        _ => Err(CommandError::Unknown),
    }
}

/// A button of the list, pressed: taking one application's permission back, or a
/// move from one page to another.
///
/// The screen is drawn again as a change to the message the button came from, and
/// Discord sends nothing back but the `custom_id` — which is why the application,
/// or the page, travelled in it. The administrator bit is asked for again here,
/// because a message stays in a channel and what a press carries is the presser's
/// own permissions, not the ones the screen was drawn with.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let guild_id = payload
        .get("guild_id")
        .and_then(as_int)
        .ok_or_else(|| CommandError::missing("grant has no guild"))?;

    if !administrator(payload)? {
        return Ok(error_screen("実行には管理者権限が必要です。"));
    }

    let pressed = crate::custom_id::ui::grant::parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    // A page button is the whole of its own answer: nothing is taken back, and the
    // screen it draws is the page it named.
    let client_id = match pressed {
        Pressed::Paged(_, number) => {
            return Ok(answered(
                page(state, guild_id, number).await?,
                UPDATE_MESSAGE,
            ));
        }
        Pressed::Revoked(client_id) => client_id,
    };

    let revoked = vc_core::grant::revoke_grant(state.pool(), &client_id, guild_id).await?;

    // Only a taking-back is told, the way the guild's approval is: a device that
    // named a webhook learns its token is dead without polling for the difference.
    // A press whose application had already lost the permission tells nobody,
    // because nothing changed.
    if let Some(application_id) = revoked {
        state
            .notifier()
            .notify_grant_decided(application_id, guild_id);
    }

    // The first page, because the button says which application it is about and
    // not which page it was on, and the row it removes moves what follows it up.
    Ok(answered(
        page(state, guild_id, FIRST_PAGE).await?,
        UPDATE_MESSAGE,
    ))
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
        vc_core::grant::Target::Guild(guild_id),
        time::OffsetDateTime::now_utc(),
    )
    .await?;

    match decided {
        Some(application_id) => {
            state
                .notifier()
                .notify_grant_decided(application_id, guild_id);

            Ok(render_ok("発行を許可しました。"))
        }
        None => Ok(render_error(
            "エラー: そのコードの申請はこのサーバーにありません。",
        )),
    }
}

/// `/grant list`: the applications this guild has allowed to issue, newest first,
/// and the button that takes each permission back.
///
/// Paged with numbers rather than a cursor, as the contract screen is: first,
/// previous, next and last are what its arrows say, and a screen of five rows
/// needs all four. The count and the rows come from one call, because reading
/// every grant in the guild to print a number is the kind of read this list is
/// small enough to avoid.
async fn page(state: &AppState, guild_id: i64, page: i64) -> Result<Value, CommandError> {
    let authorized =
        vc_core::grant::authorized_in_guild(state.pool(), guild_id, page, MAX_APPLICATIONS as i64)
            .await?;

    let mut children = Vec::new();

    if authorized.total == 0 {
        children.push(text(crate::docs::discord::mentions(
            "発行を許可しているアプリケーションはありません。申請が来たときは、\
             アプリケーションが表示するコードを `/grant approve code:` に入れて承認します。",
            state.command_ids().await,
        )));
    } else {
        children.push(text(format!(
            "**発行を許可しているアプリケーション** ({}件)\n\
             取り消すと、そのアプリケーションはこのサーバーの発行枠から発行できなくなります。",
            authorized.total
        )));

        if authorized.applications.is_empty() {
            // A page a button led to that the list has since shrunk past: the
            // arrows below are the way back, and saying where the rows are is
            // better than saying there are none.
            children.push(text(
                "このページには何もありません。前のページに戻ってください。",
            ));
        }

        for application in &authorized.applications {
            children.push(text(format!(
                "**{}**\n`{}`",
                name_of(application.client_name.as_deref()),
                application.client_id,
            )));
            children.push(action_row(vec![button(
                &revoke_custom_id(&application.client_id),
                "取り消す",
                ButtonStyle::Danger,
            )]));
        }

        if authorized.next.is_some() || authorized.page > 1 {
            children.push(pagination_row(&authorized));
        }
    }

    Ok(container(Some(COLOR_BRAND as u32), children))
}

/// Where the arrows move to, each disabled where there is nowhere to go.
fn pagination_row(authorized: &vc_core::grant::AuthorizedApplications) -> Value {
    let arrow = |at: u8, emoji: &str, target: Option<i64>, page: Page| -> Value {
        let id = match target {
            // A page that is not there is a button that says so, and one Discord
            // will not send: the id is a placeholder rather than this space's.
            None => format!("disabled-{at}"),
            Some(number) => page_custom_id(page, number),
        };

        icon_button(&id, emoji, ButtonStyle::Secondary, Some(target.is_none()))
    };

    action_row(vec![
        arrow(0, "⏪", authorized.first, Page::First),
        arrow(1, "⏮️", authorized.prev, Page::Previous),
        arrow(2, "⏭️", authorized.next, Page::Next),
        arrow(3, "⏩", authorized.last, Page::Last),
    ])
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

/// A screen as the answer to what was typed or pressed: a new ephemeral message
/// when the command asked, and `UPDATE_MESSAGE` when the answer is a redraw of
/// the message the button came from.
fn answered(screen: Value, kind: i64) -> Value {
    json!({
        "type": kind,
        "data": ephemeral(vec![screen]),
    })
}

/// A refusal, as a screen of its own: a press that cannot be answered is worth a
/// sentence rather than a broken redraw.
fn error_screen(sentence: &str) -> Value {
    json!({
        "type": UPDATE_MESSAGE,
        "data": ephemeral(vec![container(
            Some(COLOR_ERROR as u32),
            vec![text(format!("エラー: {sentence}"))],
        )]),
    })
}

/// An ephemeral answer to an approval, whose own sentence is all there is left
/// to say.
fn render_ok(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": ephemeral(vec![container(
            Some(COLOR_OK as u32),
            vec![text(content)],
        )]),
    })
}

/// An ephemeral answer with nothing to act on, which is what a refusal to the
/// command itself is: a direct message has no administrator to be, and a member
/// without the bit is not one.
fn render_error(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": ephemeral(vec![container(
            None,
            vec![text(content)],
        )]),
    })
}
