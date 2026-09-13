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
use crate::routes::oauth2_clients::{Registration, create, details, render, validated};
use crate::state::AppState;
use vc_core::application::NewApplication;

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

/// A submitted form: `dev:register` or `dev:edit`.
///
/// The values arrive as the modal's components, each keyed by the `custom_id` its input was
/// built with, and `body` is what the flows take. The edit form's flow is not extracted yet,
/// so that one still says so rather than pretending.
pub async fn modal(
    state: &AppState,
    screen: crate::custom_id::ui::developer::Screen,
    client_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let fields = submitted(payload);

    match screen {
        crate::custom_id::ui::developer::Screen::Register => {
            registration(state, body(fields), payload).await
        }
        crate::custom_id::ui::developer::Screen::Edit => {
            let target = if client_id.is_empty() {
                String::new()
            } else {
                format!(" (`{client_id}`)")
            };

            Ok(message(ephemeral(vec![crate::components::container(
                None,
                vec![crate::components::text(format!(
                    "変更の処理はまだ実装されていません{target}。"
                ))],
            )])))
        }
        // A screen with no form: reaching here means the id was built wrong.
        _ => Err(CommandError::Unknown),
    }
}

/// An application registered from a form, which is the one place it happens without a
/// browser.
///
/// The endpoint establishes its caller twice — a token says which account, and Discord is
/// then asked who that account is. Here the interaction is the caller: Discord signed it, or
/// this would not be running, so the account is the one this Discord user has and the owner
/// is the id the interaction carries. Everything after that is `create`, which is the
/// endpoint's too.
async fn registration(
    state: &AppState,
    body: Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    // Built by `body` out of what the modal asked for, so a body that will not read is this
    // side's bug rather than the caller's, and it is answered the way an internal refusal is.
    let Ok(registration) = serde_json::from_value::<Registration>(Value::Object(body)) else {
        return Ok(registered(developer::refusal("登録", None)));
    };

    let new = match validated(registration) {
        Ok(new) => new,
        Err(refusal) => return Ok(registered(developer::refusal("登録", refusal.description))),
    };

    let new = NewApplication {
        owner_discord_id: Some(me),
        ..new
    };

    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    match create(state, account, &new).await {
        Ok(created) => Ok(registered(developer::application(
            &created.client_id,
            new.client_name.as_deref(),
            false,
            None,
            Some(&created.client_secret),
        ))),
        Err(refusal) => Ok(registered(developer::refusal("登録", refusal.description))),
    }
}

/// A screen as the response to a submitted form.
fn registered(screen: Value) -> Value {
    message(ephemeral(vec![screen]))
}

/// The values a modal was submitted with, by the `custom_id` each input was built with.
///
/// Discord answers a submission with the components it was sent, the inputs one `<label>`
/// deep, which is the shape `components::label` builds and therefore the shape this reads.
fn submitted(payload: &Value) -> Vec<(String, String)> {
    fn walk(value: &Value, found: &mut Vec<(String, String)>) {
        match value {
            Value::Object(map) => {
                if let (Some(id), Some(text)) = (
                    map.get("custom_id").and_then(Value::as_str),
                    map.get("value").and_then(Value::as_str),
                ) {
                    found.push((id.to_owned(), text.to_owned()));
                }

                for nested in map.values() {
                    walk(nested, found);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, found);
                }
            }
            _ => {}
        }
    }

    let mut found = Vec::new();
    walk(payload.get("data").unwrap_or(&Value::Null), &mut found);

    found
}

/// What a form's fields are, as the body the flows take.
///
/// Both `/oauth2/clients` and the `PATCH` read their body by which fields are present, so a
/// field the form did not ask for is simply absent and is left alone. The one conversion is
/// `redirect_uris`, which is a Paragraph input: one URI per line.
fn body(fields: Vec<(String, String)>) -> serde_json::Map<String, Value> {
    let mut body = serde_json::Map::new();

    for (name, value) in fields {
        if name == "redirect_uris" {
            let uris: Vec<Value> = value
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| json!(line))
                .collect();

            body.insert(name, Value::Array(uris));
            continue;
        }

        body.insert(name, json!(value));
    }

    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The help screen is components only, on an ephemeral message, and names the
    /// subcommands — the one screen that exists without a database, so it is the one thing
    /// here that can be asserted on its own. The subcommands that read or write are tested
    /// where they are written.
    /// The shape Discord answers a modal submission with, taken from the component
    /// reference: the labels and their inputs, and the values on the inputs.
    #[test]
    fn the_submitted_values_are_read_by_their_custom_ids() {
        let payload = json!({
            "type": 5,
            "data": {
                "custom_id": "the-modal",
                "components": [
                    {
                        "type": 18,
                        "label": "クライアント名",
                        "component": {
                            "type": 4,
                            "custom_id": "client_name",
                            "value": "テスト",
                        },
                    },
                    {
                        "type": 18,
                        "label": "リダイレクト URI",
                        "component": {
                            "type": 4,
                            "custom_id": "redirect_uris",
                            "value": "https://example.test/callback\nhttps://example.test/other",
                        },
                    },
                ],
            },
        });

        assert_eq!(
            submitted(&payload),
            vec![
                ("client_name".to_owned(), "テスト".to_owned()),
                (
                    "redirect_uris".to_owned(),
                    "https://example.test/callback\nhttps://example.test/other".to_owned()
                ),
            ]
        );
    }

    /// The one conversion a form needs, and the absence that `PATCH` reads as "leave it":
    /// a field the form did not ask for is not in the body at all.
    #[test]
    fn a_form_becomes_the_body_the_flows_take() {
        let body = body(vec![
            ("client_name".to_owned(), "テスト".to_owned()),
            (
                "redirect_uris".to_owned(),
                "https://example.test/callback\n\n  https://example.test/other  ".to_owned(),
            ),
        ]);

        assert_eq!(body["client_name"], "テスト");
        assert_eq!(
            body["redirect_uris"],
            json!([
                "https://example.test/callback",
                "https://example.test/other"
            ]),
            "blank lines and padding are the form's, not the service's"
        );
        assert!(
            body.get("webhook_url").is_none(),
            "absent means untouched: {body:?}"
        );
    }

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
