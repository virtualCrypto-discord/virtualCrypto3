//! `/pat`: the credentials an account gives to something that is not a browser.
//!
//! `create` shows the credential once, privately. `list` shows names with revocation buttons.
//! Ten rows per page leave room for the controls within Discord's component limit.
//!
//! An addition rather than a port: the Elixir has no personal access token, no API key, and no
//! column that could hold one. `docs/pat.md` is the design.

use serde_json::{Map, Value, json};
use time::OffsetDateTime;

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, COLOR_OK, CommandError, UPDATE_MESSAGE,
    error_text, get_user,
};
use crate::components::{
    ButtonStyle, action_row, button, container, ephemeral, icon_button, section, text,
};
use crate::custom_id::ui::pat::{self as ids, Pressed};
use crate::docs::discord::mentions;
use crate::state::AppState;
use vc_auth::issue::{
    BROWSER_SCOPES, MAX_PERSONAL_TOKENS as MAX_TOKENS, PersonalError, personal_token,
    personal_tokens, revoke_personal,
};

/// The longest a name may be. `crate::discord_commands`'s `/pat name` option states the same
/// bound — one number in the option Discord shows and in the check that makes it true.
const NAME_MAX: usize = 32;
const PER_PAGE: usize = 10;

/// `Command.handle/4` for `pat`.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let subcommand = options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("pat has no subcommand"))?;

    let sub_options = options.get("sub_options");

    match subcommand {
        "create" => create(state, named(sub_options)?, me).await,
        "list" => list(state, me, 1, None, CHANNEL_MESSAGE_WITH_SOURCE).await,
        // Everything this service registers is written down, so a subcommand that is not is one
        // it does not have: a client with a stale command list, answered the way any unknown
        // command is.
        _ => Err(CommandError::Unknown),
    }
}

/// The required name for creation.
fn named(sub_options: Option<&Value>) -> Result<&str, CommandError> {
    sub_options
        .and_then(|options| options.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("pat option name"))
}

/// A token, once: the value, and what it can do.
///
/// The scopes come from the issuance constant so they agree with the credential.
async fn create(state: &AppState, name: &str, discord_id: i64) -> Result<Value, CommandError> {
    let Some(account) = account(state, discord_id).await? else {
        return Ok(error_screen(message!("command.pat.create.001")));
    };

    let length = name.chars().count();

    if length == 0 || length > NAME_MAX {
        return Ok(error_screen(&format!(
            message!("command.pat.create.002"),
            NAME_MAX = NAME_MAX
        )));
    }

    let now = OffsetDateTime::now_utc();

    match personal_token(
        state.pool(),
        state.jwt_secret(),
        i64::from(account),
        name,
        now,
    )
    .await
    {
        Ok(token) => Ok(screen(
            vec![
                text(format!(
                    message!("command.pat.create.003"),
                    display_name(name)
                )),
                text(format!("```\n{token}\n```")),
                text(message!("command.pat.create.004")),
                text(format!(
                    message!("command.pat.create.005"),
                    BROWSER_SCOPES.join(", ")
                )),
                text(mentions(
                    message!("command.pat.create.006"),
                    state.command_ids().await,
                )),
            ],
            COLOR_OK,
        )),
        Err(PersonalError::NameTaken(name)) => Ok(error_screen(&mentions(
            &format!(message!("command.pat.create.007"), display_name(&name)),
            state.command_ids().await,
        ))),
        Err(PersonalError::LimitReached) => Ok(error_screen(&mentions(
            &format!(message!("command.pat.create.008"), MAX_TOKENS = MAX_TOKENS),
            state.command_ids().await,
        ))),
        Err(PersonalError::Auth(error)) => Err(error.into()),
    }
}

/// Names and actions only: credential values cannot be read back.
async fn list(
    state: &AppState,
    discord_id: i64,
    number: usize,
    notice: Option<(String, i64)>,
    kind: i64,
) -> Result<Value, CommandError> {
    let Some(account) = account(state, discord_id).await? else {
        return Ok(screen(
            vec![text(message!("command.pat.list.001"))],
            COLOR_BRAND,
        ));
    };

    let tokens = personal_tokens(state.pool(), i64::from(account)).await?;

    let mut children = Vec::new();
    let mut color = COLOR_BRAND;
    if let Some((message, accent)) = notice {
        children.push(text(message));
        color = accent;
    }
    if tokens.is_empty() {
        children.push(text(mentions(
            message!("command.pat.list.002"),
            state.command_ids().await,
        )));
    } else {
        let last = tokens.len().div_ceil(PER_PAGE);
        let number = number.clamp(1, last);
        children.push(text(format!(
            message!("command.pat.list.003"),
            tokens.len(),
            MAX_TOKENS = MAX_TOKENS,
            last = last,
            number = number
        )));
        for token in tokens.iter().skip((number - 1) * PER_PAGE).take(PER_PAGE) {
            children.push(section(
                vec![text(display_name(&token.name))],
                button(
                    &ids::revoke(discord_id, number, token.token_id),
                    message!("command.pat.list.004"),
                    ButtonStyle::Danger,
                ),
            ));
        }
        if last > 1 {
            let previous = icon_button(
                &ids::page(discord_id, number - 1),
                "⏮️",
                ButtonStyle::Secondary,
                Some(number == 1),
            );
            let next = icon_button(
                &ids::page(discord_id, number + 1),
                "⏭️",
                ButtonStyle::Secondary,
                Some(number == last),
            );
            children.push(action_row(vec![previous, next]));
        }
    }
    Ok(json!({
        "type": kind,
        "data": ephemeral(vec![container(Some(color as u32), children)]),
    }))
}

/// A button never trusts its embedded owner or token id as authorization.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let discord_id =
        get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let pressed =
        ids::parse(&crate::custom_id::parse(custom_id)).map_err(|_| CommandError::Unknown)?;
    let (Pressed::Page { owner, number } | Pressed::Revoke { owner, number, .. }) = pressed;
    if owner != discord_id {
        return Ok(error_screen(message!("command.pat.component.001")));
    }
    let Pressed::Revoke { token_id, .. } = pressed else {
        return list(state, discord_id, number, None, UPDATE_MESSAGE).await;
    };
    let Some(account) = account(state, discord_id).await? else {
        return Ok(error_screen(message!("command.pat.component.002")));
    };

    let notice = match revoke_personal(state.pool(), i64::from(account), token_id).await? {
        Some(name) => (
            format!(message!("command.pat.component.003"), display_name(&name)),
            COLOR_OK,
        ),
        None => (
            message!("command.pat.component.004").to_owned(),
            COLOR_ERROR,
        ),
    };
    list(state, discord_id, number, Some(notice), UPDATE_MESSAGE).await
}

/// Keep a user-chosen name from hiding or reformatting the adjacent action.
fn display_name(name: &str) -> String {
    let mut escaped = String::new();
    for c in name.chars() {
        if "\\`*_{}[]()<>#+-.!|~".contains(c) {
            escaped.push('\\');
        }
        escaped.push(if c == '\n' || c == '\r' { ' ' } else { c });
    }
    format!("**{escaped}**")
}

/// The account behind an interaction, or nothing when the caller has none yet — a token belongs
/// to an account, so an account has to exist before one can be made.
async fn account(state: &AppState, discord_id: i64) -> Result<Option<i32>, CommandError> {
    Ok(vc_core::user::find_by_discord_id(state.pool(), discord_id)
        .await?
        .map(|user| user.id))
}

/// A refusal: the screen below in the error accent, with the heading every error screen has.
fn error_screen(sentence: &str) -> Value {
    screen(vec![text(error_text(sentence))], COLOR_ERROR)
}

/// An ephemeral screen in one colour, which is the shape every answer here has: two of them
/// carry or name a credential, so nobody else in the channel sees any of them.
fn screen(children: Vec<Value>, color: i64) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": ephemeral(vec![container(Some(color as u32), children)]),
    })
}
