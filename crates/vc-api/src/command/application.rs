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

    // Every arm answers with a whole response, because they are not all the same kind:
    // `register` and `edit` open a modal, which is its own callback type and cannot be
    // wrapped in the message one.
    match subcommand {
        "list" => Ok(message(list(state, payload).await?)),
        "show" => Ok(message(show(state, sub_options, payload).await?)),
        "register" => Ok(register()),
        "edit" => edit(state, sub_options, payload).await,
        "connect" => connect(sub_options, payload),
        "help" => Ok(message(help())),
        // The rest are registered and not written yet. Saying so is better than the answer
        // an unknown subcommand gets, because this command exists and a person pressing it
        // deserves to know which part is missing.
        other => Ok(message(ephemeral(vec![crate::components::text(format!(
            "`{other}` はまだ実装されていません。"
        ))]))),
    }
}

/// A response that shows a message. The screens are message-shaped; a modal is not.
fn message(screen: Value) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": screen,
    })
}

/// The modal that registers an application.
///
/// `register` has nothing to pre-fill and nothing to read: an application is created with
/// what this asks for.
fn register() -> Value {
    crate::components::modal(
        &crate::custom_id::ui::developer::custom_id(
            crate::custom_id::ui::developer::Screen::Register,
        ),
        "アプリケーションの登録",
        vec![
            crate::components::label(
                "クライアント名",
                Some("アプリケーションの名前です。"),
                crate::components::text_input(
                    "client_name",
                    crate::components::TextInputStyle::Short,
                    true,
                    None,
                    None,
                ),
            ),
            crate::components::label(
                "リダイレクト URI",
                Some("1行に1つ入力してください。"),
                crate::components::text_input(
                    "redirect_uris",
                    crate::components::TextInputStyle::Paragraph,
                    true,
                    Some(4000),
                    None,
                ),
            ),
            crate::components::label(
                "webhook URL",
                Some("指定すると、登録時にその URL へ確認のリクエストを送ります。"),
                crate::components::text_input(
                    "webhook_url",
                    crate::components::TextInputStyle::Short,
                    false,
                    None,
                    None,
                ),
            ),
        ],
    )
}

/// The modal that changes one, filled with what it has now.
///
/// A field that is not sent back is a field the `PATCH` clears, so every field this asks
/// for is pre-filled from the application: an edit is a correction, not a retyping.
async fn edit(
    state: &AppState,
    sub_options: Option<&Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let Some(found) = owned_application(state, sub_options, payload).await? else {
        return Ok(message(ephemeral(vec![crate::components::text(
            "そのアプリケーションはありません。".to_string(),
        )])));
    };

    let redirect_uris = found.redirect_uris.join("\n");

    Ok(crate::components::modal(
        &crate::custom_id::ui::developer::custom_id(crate::custom_id::ui::developer::Screen::Edit),
        "アプリケーションの設定",
        vec![
            crate::components::label(
                "クライアント名",
                None,
                crate::components::text_input(
                    "client_name",
                    crate::components::TextInputStyle::Short,
                    true,
                    None,
                    found.client_name.as_deref(),
                ),
            ),
            crate::components::label(
                "リダイレクト URI",
                Some("1行に1つ入力してください。"),
                crate::components::text_input(
                    "redirect_uris",
                    crate::components::TextInputStyle::Paragraph,
                    true,
                    Some(4000),
                    Some(&redirect_uris),
                ),
            ),
            crate::components::label(
                "クライアント URI",
                None,
                crate::components::text_input(
                    "client_uri",
                    crate::components::TextInputStyle::Short,
                    false,
                    None,
                    found.client_uri.as_deref(),
                ),
            ),
            crate::components::label(
                "ロゴ URI",
                None,
                crate::components::text_input(
                    "logo_uri",
                    crate::components::TextInputStyle::Short,
                    false,
                    None,
                    found.logo_uri.as_deref(),
                ),
            ),
            crate::components::label(
                "webhook URL",
                None,
                crate::components::text_input(
                    "webhook_url",
                    crate::components::TextInputStyle::Short,
                    false,
                    None,
                    found.webhook_url.as_deref(),
                ),
            ),
        ],
    ))
}

/// The caller's own application that this interaction names, or nothing.
///
/// The ownership check is the connect route's, in the same order, and it is here once for
/// `show` and `edit` both.
async fn owned_application(
    state: &AppState,
    sub_options: Option<&Value>,
    payload: &Value,
) -> Result<Option<crate::routes::oauth2_clients::Details>, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;
    let client_id = client_id_of(sub_options)?;

    let owned = vc_core::application::owned_by(state.pool(), account).await?;

    crate::routes::connect::owned_by_client_id(state.pool(), &owned, client_id)
        .await
        .map_err(CommandError::from)
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

    // Every screen is one container, which is not only for looks: the schema the tests
    // validate against rejects a bare Text Display at the top level of a message, and it
    // caught these two doing it.
    let body = if payload.get("guild_id").and_then(Value::as_str).is_none() {
        "Bot の接続はサーバーの中で行います。接続したいサーバーで `/application connect` を実行してください。"
    } else {
        "接続の処理はまだ実装されていません。"
    };

    Ok(message(ephemeral(vec![crate::components::container(
        None,
        vec![crate::components::text(body)],
    )])))
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
    let Some(found) = owned_application(state, sub_options, payload).await? else {
        return Ok(ephemeral(vec![crate::components::text(
            "そのアプリケーションはありません。".to_string(),
        )]));
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
    ephemeral(vec![crate::components::container(
        None,
        vec![
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
        ],
    )])
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

        // Every screen is one container, and the copy is somewhere inside it: asserting on
        // the whole thing as text is what keeps this from breaking each time the tree
        // gains a level, which it has twice.
        let body = screen["components"].to_string();

        for subcommand in ["list", "show", "register", "edit", "connect", "secret"] {
            assert!(body.contains(subcommand), "{subcommand} is not in {body}");
        }
    }
}
