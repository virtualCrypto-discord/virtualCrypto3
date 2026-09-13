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

use super::{CHANNEL_MESSAGE_WITH_SOURCE, CommandError, UPDATE_MESSAGE, get_user, option_text};
use crate::components::ephemeral;
use crate::developer;
use crate::routes::oauth2_clients::{
    Registration, apply, changes, create, details, render, validated,
};
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
        "show" => Ok(message(
            show(state, client_id_of(sub_options)?, payload).await?,
        )),
        "register" => Ok(register()),
        "edit" => edit(state, client_id_of(sub_options)?, payload).await,
        "connect" => connect(state, sub_options, payload).await,
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
async fn edit(state: &AppState, client_id: &str, payload: &Value) -> Result<Value, CommandError> {
    let Some((_, found)) = owned(state, client_id, payload).await? else {
        return Ok(message(ephemeral(vec![developer::plain(
            "そのアプリケーションはありません。",
        )])));
    };

    let redirect_uris = found.redirect_uris.join("\n");

    Ok(crate::components::modal(
        // The application is named in the id, because a submission comes back with the id it
        // was opened with and nothing else — without this the form could not say what it was
        // editing, and would edit whatever the id happened to name, which was nothing.
        &crate::custom_id::ui::developer::custom_id_for(
            crate::custom_id::ui::developer::Screen::Edit,
            &found.client_id,
        ),
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

/// The caller's own application that this `client_id` names, with its id.
///
/// The ownership check is the connect route's, in the same order, and it is here once for the
/// three places that look at one application: the screen, the form that changes it, and the
/// answer to that form.
async fn owned(
    state: &AppState,
    client_id: &str,
    payload: &Value,
) -> Result<Option<(i64, crate::routes::oauth2_clients::Details)>, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

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
/// The proof the flow needs is Discord's rather than the caller's: it reads the guild's
/// integrations and insists one of them is this bot and says so in its description. Nothing
/// here can assert ownership, which is why this is the same `connect_application` the route
/// calls, with the guild the interaction already carries instead of one somebody pasted.
/// Connecting a bot, once it is known which bot.
///
/// Where that came from is the caller's business — the option a subcommand was given, or the
/// picker on the application's screen — and everything after it is the same flow, which is the
/// point. The guild is always the interaction's: connecting from a DM would mean asking for a
/// guild id to be typed, and that typing is what the web page's form did badly.
async fn connect_bot(
    state: &AppState,
    client_id: &str,
    bot: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let Some(guild) = payload.get("guild_id").and_then(Value::as_str) else {
        return Ok(developer::plain(
            "Bot の接続はサーバーの中で行います。接続したいサーバーで `/application connect` を実行してください。",
        ));
    };

    // Discord sends every id as text and this is the same single parse the route makes. The
    // text is kept as well because it is what Discord's own answer is compared against.
    let (Ok(bot_id), Ok(guild_id)) = (bot.parse::<i64>(), guild.parse::<i64>()) else {
        return Err(CommandError::missing(
            "the bot or the guild is not a Discord id",
        ));
    };

    let Some((_, found)) = owned(state, client_id, payload).await? else {
        return Ok(developer::plain("そのアプリケーションはありません。"));
    };

    match crate::routes::connect::connect_application(state, &found, bot, bot_id, guild_id).await {
        Ok(()) => Ok(developer::connect_result(&found.client_id, None)),
        Err(refusal) => Ok(match refusal.description.as_deref() {
            Some(description) => developer::connect_result(&found.client_id, Some(description)),
            // A failure with no description is this service's, and `connect_result` cannot say
            // that: its `None` is the success this just was not.
            None => developer::refusal("接続", None),
        }),
    }
}

/// `/application connect`, whose bot is one of its options.
async fn connect(
    state: &AppState,
    sub_options: Option<&Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let client_id = client_id_of(sub_options)?;

    let bot = match sub_options.and_then(Value::as_object) {
        Some(options) => option_text(options, "bot")?,
        // The option is required and its picker only answers with users, so this is this side's
        // assumption failing rather than something a caller did.
        None => return Err(CommandError::missing("connect was given no bot")),
    };

    Ok(message(ephemeral(vec![
        connect_bot(state, client_id, &bot, payload).await?,
    ])))
}

/// One application, with its secret on the screen.
///
/// The ownership check is the one the connect route makes, and in the same order: the
/// caller's own ids first, and only then a `client_id` looked for among them. Checking the
/// uuid first would answer whether an application exists to somebody who owns none.
async fn show(state: &AppState, client_id: &str, payload: &Value) -> Result<Value, CommandError> {
    let Some((_, found)) = owned(state, client_id, payload).await? else {
        return Ok(ephemeral(vec![developer::plain(
            "そのアプリケーションはありません。",
        )]));
    };

    Ok(ephemeral(vec![developer::application(
        &found.client_id,
        found.client_name.as_deref(),
        found.discord_user_id.is_some(),
        found.logo_uri.as_deref(),
        found.client_secret.as_deref(),
        developer::Choices {
            application_type: &found.application_type,
            grant_types: &found.grant_types,
            response_types: &found.response_types,
        },
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
            edit_form(state, client_id, body(fields), payload).await
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
        Err(refusal) => {
            return Ok(registered(developer::refusal(
                "登録",
                refusal.description.as_deref(),
            )));
        }
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
            developer::Choices {
                application_type: &new.application_type,
                grant_types: &new.grant_types,
                // Registration writes `response_types` as the literal empty list — the Elixir
                // validates it and then does not use it, which docs/oauth2.md records — so a
                // freshly registered application has none until an edit sets them.
                response_types: &[],
            },
        ))),
        Err(refusal) => Ok(registered(developer::refusal(
            "登録",
            refusal.description.as_deref(),
        ))),
    }
}

/// A change submitted from a form.
///
/// The endpoint insists on an application token carrying `oauth2.register`, because that is
/// all a `PATCH` has to go on — the token's subject *is* the application being written. Here
/// the application is the one the `custom_id` names and the interaction says who is asking,
/// so ownership is what authorizes it. What is left is the same `changes` and `apply` the
/// endpoint uses, and the same write.
async fn edit_form(
    state: &AppState,
    client_id: &str,
    body: Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let Some((application_id, found)) = owned(state, client_id, payload).await? else {
        return Ok(registered(developer::refusal(
            "変更",
            Some("そのアプリケーションはありません。"),
        )));
    };

    let changes = match changes(&body) {
        Ok(changes) => changes,
        Err(refusal) => {
            return Ok(registered(developer::refusal(
                "変更",
                refusal.description.as_deref(),
            )));
        }
    };

    // The application's own account, which is what the endpoint's token subject is, so the
    // handshake budget is the application's either way.
    let key = found.user_id.to_string();

    match apply(state, application_id, &key, &changes).await {
        Ok(()) => match details(state.pool(), application_id).await {
            Ok(Some(now)) => Ok(registered(developer::application(
                &now.client_id,
                now.client_name.as_deref(),
                now.discord_user_id.is_some(),
                now.logo_uri.as_deref(),
                now.client_secret.as_deref(),
                developer::Choices {
                    application_type: &now.application_type,
                    grant_types: &now.grant_types,
                    response_types: &now.response_types,
                },
            ))),
            _ => Ok(registered(developer::refusal("変更", None))),
        },
        Err(refusal) => Ok(registered(developer::refusal(
            "変更",
            refusal.description.as_deref(),
        ))),
    }
}

/// A component on one of these screens: a button, or the menu's select.
///
/// A button that opens a form answers with the form — a modal is its own callback type and
/// cannot be wrapped in a message. Everything else answers with the screen it goes to, as an
/// *update*, so pressing 戻る replaces the panel rather than stacking another one in the DM,
/// which is what a person pressing 戻る means.
///
/// The screens that are not here are the ones whose data this does not have: `home` says
/// 接続しています to a person by name and links to a page, and neither the name nor the page is
/// in the interaction or in [`crate::state::Links`]; `connect` is the HTTP route's flow, which
/// is not extracted yet. They answer `Unknown`, which the dispatcher turns into the refusal an
/// unhandled component already gets, rather than a screen that pretends.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    use crate::custom_id::ui::developer::{Screen, parse};

    let (screen, client_id) =
        parse(&crate::custom_id::parse(custom_id)).map_err(|_| CommandError::Unknown)?;

    let data = payload.get("data");
    let component_type = data
        .and_then(|data| data.get("component_type"))
        .and_then(Value::as_i64);

    match (component_type, screen) {
        // The list's menu. Its id says which screen the menu is on rather than what choosing
        // does, so the value that came back is the thing to act on, and it is a `client_id`.
        // A field's own menu: the id says which application and which field, and the values
        // that came back are the set the person chose. This comes first because the arm below
        // takes any string select and that one is the list's menu, not a field's.
        (Some(3), Screen::Edit) => {
            let (client_id, field) = crate::custom_id::ui::developer::field_of(&client_id);

            let Some((application_id, found)) = owned(state, client_id, payload).await? else {
                return Err(CommandError::Unknown);
            };

            let values = data
                .and_then(|data| data.get("values"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            let Some((field, value)) = edited(field, values) else {
                return Err(CommandError::Unknown);
            };

            let mut body = Map::new();
            body.insert(field, value);

            let changes = match changes(&body) {
                Ok(changes) => changes,
                Err(refusal) => {
                    return Ok(update(ephemeral(vec![developer::refusal(
                        "変更",
                        refusal.description.as_deref(),
                    )])));
                }
            };

            // The application's own account, which is what the endpoint's token subject is, so
            // the handshake budget is the application's either way.
            let key = found.user_id.to_string();

            match apply(state, application_id, &key, &changes).await {
                // The screen again, so the person sees the set they now have rather than the one
                // they chose — which is the same thing only when the write agreed with them.
                Ok(()) => Ok(update(show(state, client_id, payload).await?)),
                Err(refusal) => Ok(update(ephemeral(vec![developer::refusal(
                    "変更",
                    refusal.description.as_deref(),
                )]))),
            }
        }
        // The list's menu, whose value is the application to look at.
        (Some(3), _) => Ok(update(show(state, chosen(data)?, payload).await?)),
        // The bot picker on an application's screen, whose value is the bot that was chosen.
        (Some(5), Screen::Connect) => {
            let bot = chosen(data)?;

            Ok(update(ephemeral(vec![
                connect_bot(state, &client_id, bot, payload).await?,
            ])))
        }
        // The list, from a screen that is not it: the one button left that goes anywhere.
        (Some(2), Screen::List | Screen::Back) => Ok(update(list(state, payload).await?)),
        _ => Err(CommandError::Unknown),
    }
}

/// One field, as the request takes it.
///
/// Two of these fields are sets and one is a single value, and the difference is the endpoint's:
/// `grant_types` and `response_types` are read as arrays and `application_type` as a string. So
/// a single choice is unwrapped here rather than sent as an array the endpoint would refuse.
fn edited(field: &str, values: Vec<Value>) -> Option<(String, Value)> {
    match field {
        "application_type" => values
            .first()
            .map(|value| (field.to_owned(), value.clone())),
        "grant_types" | "response_types" => Some((field.to_owned(), Value::Array(values))),
        // Anything else is a field with no menu, which means an id this module did not build.
        _ => None,
    }
}

/// The one value a menu came back with.
///
/// Every menu here is a single choice — one application to look at, one bot to connect — so the
/// first value is the answer, and more than one would be a component this module did not build.
fn chosen(data: Option<&Value>) -> Result<&str, CommandError> {
    data.and_then(|data| data.get("values"))
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or(CommandError::Unknown)
}

/// A screen as a change to the message that was already there.
fn update(screen: Value) -> Value {
    json!({
        "type": UPDATE_MESSAGE,
        "data": screen,
    })
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
/// field the form did not ask for is simply absent and is left alone.
///
/// An empty box is not absence. Discord sends `""` for one, and an empty string is a value —
/// a URL that is not one, a webhook that cannot be verified — so it becomes `null`, which is
/// what the `PATCH` reads as "clear it" and what somebody who emptied the box meant. The
/// edit form pre-fills every field it asks for, so a field that still has its value comes
/// back with it and is written back unchanged.
///
/// The other conversion is `redirect_uris`, which is a Paragraph input: one URI per line.
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

        if value.is_empty() {
            body.insert(name, Value::Null);
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

    /// The two conversions a form needs: lines into a list, and an empty box into a null
    /// rather than an empty string — `PATCH` leaves an absent field alone and clears a null
    /// one, and an empty string would be a URL that is not one.
    #[test]
    fn a_form_becomes_the_body_the_flows_take() {
        let body = body(vec![
            ("client_name".to_owned(), "テスト".to_owned()),
            (
                "redirect_uris".to_owned(),
                "https://example.test/callback\n\n  https://example.test/other  ".to_owned(),
            ),
            ("webhook_url".to_owned(), String::new()),
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
        assert_eq!(
            body["webhook_url"],
            Value::Null,
            "an emptied box is a value somebody meant to remove"
        );
        assert!(
            body.get("client_uri").is_none(),
            "a field the form does not ask for is absent, which means untouched: {body:?}"
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
