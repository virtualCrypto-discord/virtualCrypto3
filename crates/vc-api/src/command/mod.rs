//! The slash-command handlers and the responses they render.
//!
//! `VirtualCryptoWeb.Interaction.Command.handle/4` returns a term that
//! `VirtualCryptoWeb.Api.InteractionsJSON` renders. Rust has no view layer, so
//! the two steps are one here and a handler returns the interaction response
//! directly; the renderers stay separate functions so they still line up with
//! `InteractionsJSON`.

pub mod application;
pub mod autocomplete;
mod bal;
pub mod claim;
pub mod create;
pub mod delete;
pub mod give;
pub mod info;
pub mod pay;

use serde_json::{Map, Value, json};

use crate::error::ApiError;
use crate::state::AppState;

/// `Interactions.Util`: response types and colours.
pub const PONG: i64 = 1;
pub const CHANNEL_MESSAGE_WITH_SOURCE: i64 = 4;
pub const UPDATE_MESSAGE: i64 = 7;
pub const MODAL: i64 = 9;
pub const AUTOCOMPLETE_RESULT: i64 = 8;
pub const EPHEMERAL: i64 = 64;
pub const COLOR_OK: i64 = 0x38EA42;
pub const COLOR_ERROR: i64 = 0xEA3875;
pub const COLOR_BRAND: i64 = 0x6221ED;

/// `Interactions.Util`: component types and their styles.
pub const ACTION_ROW: i64 = 1;
pub const BUTTON: i64 = 2;
pub const SELECT_MENU: i64 = 3;
pub const TEXT_INPUT: i64 = 4;
pub const BUTTON_STYLE_PRIMARY: i64 = 1;
pub const BUTTON_STYLE_SECONDARY: i64 = 2;
pub const BUTTON_STYLE_SUCCESS: i64 = 3;
pub const BUTTON_STYLE_DANGER: i64 = 4;
pub const BUTTON_STYLE_LINK: i64 = 5;
pub const TEXT_INPUT_STYLE_SHORT: i64 = 1;
pub const TEXT_INPUT_STYLE_PARAGRAPH: i64 = 2;

/// Why a command produced no response.
#[derive(Debug)]
pub enum CommandError {
    /// No `Command.handle/4` clause matches this name, so the controller never
    /// reaches a renderer. Reported as `Type Not Found`.
    Unknown,
    /// The corresponding Elixir code raises here — a lookup failed, or a value
    /// it assumed was present is missing — which Phoenix turns into a 500.
    Internal(ApiError),
}

impl CommandError {
    /// The Elixir handlers pattern-match on values they assume are present.
    pub fn missing(what: &str) -> Self {
        CommandError::Internal(ApiError::Internal(what.to_string()))
    }
}

impl From<vc_core::Error> for CommandError {
    fn from(error: vc_core::Error) -> Self {
        CommandError::Internal(ApiError::from(error))
    }
}

impl From<sqlx::Error> for CommandError {
    fn from(error: sqlx::Error) -> Self {
        CommandError::Internal(ApiError::from(vc_core::Error::Database(error)))
    }
}

impl From<crate::discord::DiscordError> for CommandError {
    fn from(error: crate::discord::DiscordError) -> Self {
        CommandError::Internal(ApiError::from(error))
    }
}

/// `Interactions.Util.mention/1`.
pub fn mention(id: impl std::fmt::Display) -> String {
    format!("<@{id}>")
}

/// `Command.continue_management_command?/2`, with the branch it used to take
/// removed.
///
/// Elixir only demanded the administrator bit from guilds carrying
/// `APPLICATION_COMMAND_PERMISSIONS_V2`, and let every other guild through.
/// Discord finished that migration, so every guild carries it and the branch
/// can no longer be false — it is gone, and with it the guild lookup both
/// callers used to make just to read the feature list.
pub fn is_administrator(permissions: u64) -> bool {
    const ADMINISTRATOR: u64 = 0x8;

    permissions & ADMINISTRATOR == ADMINISTRATOR
}

/// `Command.cast_int/1`, and what `String.to_integer/1` does to an option.
/// Discord sends ids and integer options as strings, but several callers send
/// numbers, so both are accepted.
pub fn as_int(value: &Value) -> Option<i64> {
    match value {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_i64(),
        _ => None,
    }
}

/// Discord sends a permission bit set as a decimal string, and it needs all 64
/// bits: the fixtures use `0xFFFFFFFFFFFFFFFF`, which no `i64` holds.
pub fn as_permissions(value: &Value) -> Option<u64> {
    match value {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

/// `InteractionsController.parse_options/1`.
///
/// A subcommand arrives as a nested `options` list and becomes `subcommand` plus
/// `sub_options`; every other option becomes a plain entry, with string values
/// trimmed. Later entries win, because the Elixir version flattens the list and
/// builds a map with `Map.new/1`.
pub fn parse_options(options: &[Value]) -> Map<String, Value> {
    let mut parsed = Map::new();

    for option in options {
        let Some(name) = option.get("name").and_then(Value::as_str) else {
            continue;
        };

        if let Some(sub) = option.get("options").and_then(Value::as_array) {
            parsed.insert("subcommand".to_string(), Value::String(name.to_string()));
            parsed.insert("sub_options".to_string(), Value::Object(parse_options(sub)));
        } else if let Some(value) = option.get("value") {
            let value = match value {
                Value::String(text) => Value::String(text.trim().to_string()),
                other => other.clone(),
            };

            parsed.insert(name.to_string(), value);
        } else {
            parsed.insert("subcommand".to_string(), Value::String(name.to_string()));
        }
    }

    parsed
}

/// `Interaction.Util.get_user/1`: the invoking user is `member.user` in a guild
/// and `user` in a direct message.
pub fn get_user(payload: &Value) -> Option<i64> {
    payload
        .get("member")
        .and_then(|member| member.get("user"))
        .or_else(|| payload.get("user"))
        .and_then(|user| user.get("id"))
        .and_then(as_int)
}

/// A required option, as text. Elixir takes these out of the parsed map with a
/// `handle/4` head, so a missing one is a function-clause error rather than a
/// client error.
pub fn option_text(options: &Map<String, Value>, name: &str) -> Result<String, CommandError> {
    match options.get(name) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(other) => Ok(other.to_string()),
        None => Err(CommandError::missing(&format!("option {name}"))),
    }
}

/// How an option's value is interpolated into a message: `"#{value}"` in Elixir,
/// so a string stays as it is and a number is written out in full.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `Command.handle/4`: the command name picks a handler.
pub async fn handle(
    state: &AppState,
    name: &str,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    match name {
        "help" => Ok(help(state)),
        "invite" => Ok(invite(state)),
        "application" => application::handle(state, options, payload).await,
        "bal" => bal::handle(state, payload).await,
        "claim" => claim::handle(state, options, payload).await,
        "create" => create::handle(state, options, payload).await,
        "delete" => delete::handle(state, options, payload).await,
        "give" => give::handle(state, options, payload).await,
        "info" => info::handle(state, options, payload).await,
        "pay" => pay::handle(state, options, payload).await,
        _ => Err(CommandError::Unknown),
    }
}

/// `Command.handle/4` for `help`, rendered by `InteractionsJSON.help/1`.
pub fn help(state: &AppState) -> Value {
    let links = state.links();

    let description = format!(
        "VirtualCryptoはDiscord上でサーバーに独自の通貨を作成できるBotです。\n\
         [コマンドの使い方の詳細]({site}/document/commands)\n\
         [公式サイト]({site})\n\
         [Botの招待]({bot})\n\
         [サポートサーバーの招待]({support})",
        site = links.site_url,
        bot = links.invite_url,
        support = links.support_guild_invite_url,
    );

    embed(description, links.logo_url())
}

/// `Command.handle/4` for `invite`, rendered by `InteractionsJSON.invite/1`.
pub fn invite(state: &AppState) -> Value {
    let links = state.links();

    let description = format!(
        "[Botの招待]({bot})\n[サポートサーバーの招待]({support})",
        bot = links.invite_url,
        support = links.support_guild_invite_url,
    );

    embed(description, links.logo_url())
}

/// The ephemeral, brand-coloured embed both commands share.
fn embed(description: String, logo_url: String) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "embeds": [{
                "color": COLOR_BRAND,
                "title": "VirtualCrypto",
                "thumbnail": { "url": logo_url },
                "description": description,
            }],
        },
    })
}
