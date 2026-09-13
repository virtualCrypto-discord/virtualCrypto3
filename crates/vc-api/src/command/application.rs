//! `/application`: the developer features, as subcommands.
//!
//! Each subcommand answers on an ephemeral, components-only message — see
//! [`crate::developer`] for the screens themselves, which are data and have no idea an
//! interaction exists.
//!
//! The arguments come from Discord: a typed option, its own picker for a user, and
//! autocomplete for a `client_id`, which [`crate::command::autocomplete`] fills from the
//! caller's own applications. Nothing here parses a snowflake or a uuid out of a string.

use serde_json::{Map, Value, json};

use super::{CHANNEL_MESSAGE_WITH_SOURCE, CommandError, get_user};
use crate::components::ephemeral;
use crate::developer;
use crate::routes::oauth2_clients::{details, render};
use crate::state::AppState;

/// The subcommand that was run, and its own options if it has any.
///
/// The dispatcher has already taken the option tree apart into `subcommand` and
/// `sub_options`, which is the shape `claim::handle` reads too.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let subcommand = options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("application has no subcommand"))?;

    let sub_options = options.get("sub_options");

    let screen = match subcommand {
        "list" => list(state, payload).await?,
        "show" => show(state, sub_options, payload).await?,
        "connect" => connect(sub_options, payload)?,
        "help" => help(),
        // The rest are registered and not written yet. Saying so is better than the
        // answer an unknown subcommand gets, because this command exists and a person
        // pressing it deserves to know which part is missing.
        other => ephemeral(vec![crate::components::text(format!(
            "`{other}` はまだ実装されていません。"
        ))]),
    };

    Ok(json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": screen,
    }))
}

/// `/application connect`, which is a guild's to run.
///
/// Connecting a bot to a server from a DM means asking somebody to paste a guild id, which
/// is the most error-prone thing in this feature and exactly what the web page's form does
/// badly. In a guild the id is already in the interaction, so there is nothing to type —
/// and if this is run in a DM anyway, saying where to run it is better than silence.
///
/// The flow itself is the HTTP route's, `routes/connect.rs`, and it is not extracted yet,
/// so the guild branch says that rather than pretending.
fn connect(sub_options: Option<&Value>, payload: &Value) -> Result<Value, CommandError> {
    let _ = client_id_of(sub_options)?;

    if payload.get("guild_id").and_then(Value::as_str).is_none() {
        return Ok(ephemeral(vec![crate::components::text(
            "Bot の接続はサーバーの中で行います。接続したいサーバーで              `/application connect` を実行してください。",
        )]));
    }

    Ok(ephemeral(vec![crate::components::text(
        "接続の処理はまだ実装されていません。",
    )]))
}

/// One application, with its secret on the screen.
///
/// The ownership check is the one the connect route makes, and in the same order: the
/// caller's own ids first, and only then a `client_id` looked for among them. Checking the
/// uuid first would answer whether an application exists to somebody who owns none.
async fn show(
    state: &AppState,
    sub_options: Option<&Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;
    let client_id = client_id_of(sub_options)?;

    let owned = vc_core::application::owned_by(state.pool(), account).await?;

    let found = crate::routes::connect::owned_by_client_id(state.pool(), &owned, client_id)
        .await
        .map_err(vc_core::Error::from)?;

    let Some(found) = found else {
        return Ok(ephemeral(vec![crate::components::text(format!(
            "`{client_id}` は見つかりませんでした。"
        ))]));
    };

    Ok(ephemeral(vec![developer::application(
        &found.client_id,
        found.client_name.as_deref(),
        found.discord_user_id.is_some(),
        found.logo_uri.as_deref(),
        found.client_secret.as_deref(),
    )]))
}

/// The `client_id` a subcommand was given, which autocomplete filled from the caller's own.
fn client_id_of(sub_options: Option<&Value>) -> Result<&str, CommandError> {
    sub_options
        .and_then(|options| options.get("client_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("this subcommand needs a client_id"))
}

/// What the caller owns, as the menu [`developer::applications`] draws.
async fn list(state: &AppState, payload: &Value) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    let mut owned = Vec::new();

    for application_id in vc_core::application::owned_by(state.pool(), account).await? {
        // The same render the HTTP reads answer with, so the menu and the page cannot
        // disagree about what an application is.
        let found = details(state.pool(), application_id)
            .await
            .map_err(vc_core::Error::from)?;

        if let Some(found) = found {
            owned.push(render(&found));
        }
    }

    Ok(ephemeral(vec![developer::applications(&owned)]))
}

fn help() -> Value {
    ephemeral(vec![
        crate::components::text(
            "**/application**\n\n\
             `list` 自分が持つアプリケーション\n\
             `show <client_id>` アプリケーションの詳細\n\
             `register` 新しいアプリケーションを登録\n\
             `edit <client_id>` 設定の変更\n\
             `connect <client_id> <bot>` サーバーに Bot を接続\n\
             `secret <client_id>` client_secret の表示\n\n\
             `client_id` は入力しながら候補から選べます。",
        ),
        crate::components::action_row(vec![crate::components::button(
            "dev:list",
            "アプリケーション",
            crate::components::ButtonStyle::Primary,
        )]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The help screen is components only, on an ephemeral message, and names the
    /// subcommands — the one screen that exists without a database, so it is the one thing
    /// here that can be asserted on its own. The subcommands that read or write are tested
    /// where they are written.
    #[test]
    fn help_is_an_ephemeral_component_message_that_names_the_subcommands() {
        let screen = help();

        assert_eq!(screen["flags"], json!(32832));
        assert_eq!(screen["content"], Value::Null);

        let body = screen["components"][0]["content"].as_str().expect("text");

        for subcommand in ["list", "show", "register", "edit", "connect", "secret"] {
            assert!(body.contains(subcommand), "{subcommand} is not in {body}");
        }
    }
}
