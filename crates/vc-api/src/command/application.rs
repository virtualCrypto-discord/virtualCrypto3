//! `/application`: the developer features, as subcommands.
//!
//! Each subcommand answers on an ephemeral, components-only message — see
//! [`crate::developer`] for the screens themselves, which are data and have no idea an
//! interaction exists.
//!
//! The arguments come from Discord: a typed option, its own picker for a user, and
//! autocomplete for a `client_id`, which [`crate::command::autocomplete`] fills from the
//! caller's own applications. Nothing here parses a snowflake or a uuid out of a string.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use std::time::Duration;

use super::{CHANNEL_MESSAGE_WITH_SOURCE, CommandError, UPDATE_MESSAGE, get_user};
use crate::components::ephemeral;
use crate::developer;
use crate::routes::oauth2_clients::{
    Registration, apply, changes, create, details, render, validated,
};
use crate::state::AppState;
use vc_core::application::{NewApplication, TextField};

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

    // Registration creates the application and returns its credentials privately.
    match subcommand {
        "list" => Ok(message(list(state, payload).await?)),
        "show" => Ok(message(
            show(state, client_id_of(sub_options)?, payload).await?,
        )),
        "register" => register(state, payload).await,
        // Everything registered is written, so an unknown subcommand is one this service does
        // not have — a client with a stale command list, answered the way any unknown command
        // is.
        _ => Err(CommandError::Unknown),
    }
}

/// A response that shows a message. The screens are message-shaped; a modal is not.
fn message(screen: Value) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": screen,
    })
}

/// An application made with defaults, answered with the screen that edits it.
///
/// The defaults are this command's rather than the endpoint's, and two of them are deliberate:
/// `grant_types` absent means *none*, which makes an application that can do nothing, so this
/// asks for the one grant type an authorization code flow needs; and `webhook_url` is absent
/// because a value there sends a handshake to somebody else's server in the middle of a
/// registration. Nothing is asked for, so nothing has to be typed — the screen is where the
/// nine fields are set, and it is the same nine controls whether the application is new or old.
async fn register(state: &AppState, payload: &Value) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let registration = Registration {
        application_type: Some("web".to_owned()),
        grant_types: Some(vec!["authorization_code".to_owned()]),
        response_types: Some(vec!["code".to_owned()]),
        // Present and empty: absent is what the endpoint refuses, and an application with no
        // redirect URI yet is one that is still being set up.
        redirect_uris: Some(Vec::new()),
        ..Registration::default()
    };

    let new = match validated(registration) {
        Ok(new) => new,
        Err(refusal) => {
            return Ok(message(ephemeral(vec![developer::refusal(
                message!("command.application.register.001"),
                refusal.description.as_deref(),
            )])));
        }
    };

    let new = NewApplication {
        owner_discord_id: Some(me),
        ..new
    };

    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    match create(state, account, &new).await {
        Ok(created) => Ok(message(ephemeral(vec![developer::saved(
            developer::application(
                &created.client_id,
                new.client_name.as_deref(),
                None,
                None,
                Some(&created.client_secret),
                &crate::routes::connect::token_url(state.links(), &created.client_id),
                developer::Fields {
                    client_name: new.client_name.as_deref(),
                    redirect_uris: &new.redirect_uris,
                    client_uri: new.client_uri.as_deref(),
                    logo_uri: new.logo_uri.as_deref(),
                    webhook_url: new.webhook_url.as_deref(),
                    discord_support_server_invite_slug: new
                        .discord_support_server_invite_slug
                        .as_deref(),
                    application_type: &new.application_type,
                    grant_types: &new.grant_types,
                    response_types: &new.response_types,
                    subscribed_events: &new.subscribed_events,
                },
            ),
            message!("command.application.register.002"),
        )]))),
        Err(refusal) => Ok(message(ephemeral(vec![developer::refusal(
            message!("command.application.register.003"),
            refusal.description.as_deref(),
        )]))),
    }
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

/// Connecting a bot, which a guild's screen is the way to ask for.
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
        return Ok(guild_required(state).await);
    };

    // Discord sends every id as text and this is the same single parse the route makes. The
    // text is kept as well because it is what Discord's own answer is compared against.
    let (Ok(bot_id), Ok(guild_id)) = (bot.parse::<i64>(), guild.parse::<i64>()) else {
        return Err(CommandError::missing(
            "the bot or the guild is not a Discord id",
        ));
    };

    let Some((_, found)) = owned(state, client_id, payload).await? else {
        return Ok(developer::error(message!(
            "command.application.connect_bot.001"
        )));
    };

    match crate::routes::connect::connect_application(state, &found, bot, bot_id, guild_id).await {
        Ok(()) => Ok(developer::connect_result(&found.client_id, None)),
        Err(refusal) => Ok(match refusal.description.as_deref() {
            Some(description) => developer::connect_result(&found.client_id, Some(description)),
            // A failure with no description is this service's, and `connect_result` cannot say
            // that: its `None` is the success this just was not.
            None => developer::refusal(message!("command.application.connect_bot.002"), None),
        }),
    }
}

async fn guild_required(state: &AppState) -> Value {
    developer::error(&crate::docs::discord::mentions(
        message!("command.application.guild_required.001"),
        state.command_ids().await,
    ))
}

/// One application, with its secret on the screen.
///
/// The ownership check is the one the connect route makes, and in the same order: the
/// caller's own ids first, and only then a `client_id` looked for among them. Checking the
/// uuid first would answer whether an application exists to somebody who owns none.
async fn show(state: &AppState, client_id: &str, payload: &Value) -> Result<Value, CommandError> {
    let Some((_, found)) = owned(state, client_id, payload).await? else {
        // A refusal rather than a sentence: it carries the list button, and a screen with no way
        // on is the same problem as a button that goes nowhere.
        return Ok(ephemeral(vec![developer::refusal(
            message!("command.application.show.001"),
            Some(message!("command.application.show.002")),
        )]));
    };

    Ok(ephemeral(vec![application_screen(state, &found)]))
}

fn application_screen(state: &AppState, found: &crate::routes::oauth2_clients::Details) -> Value {
    developer::application(
        &found.client_id,
        found.client_name.as_deref(),
        found.discord_user_id,
        found.logo_uri.as_deref(),
        found.client_secret.as_deref(),
        &crate::routes::connect::token_url(state.links(), &found.client_id),
        developer::Fields {
            client_name: found.client_name.as_deref(),
            redirect_uris: &found.redirect_uris,
            client_uri: found.client_uri.as_deref(),
            logo_uri: found.logo_uri.as_deref(),
            webhook_url: found.webhook_url.as_deref(),
            discord_support_server_invite_slug: found.discord_support_server_invite_slug.as_deref(),
            application_type: &found.application_type,
            grant_types: &found.grant_types,
            response_types: &found.response_types,
            subscribed_events: &found.subscribed_events,
        },
    )
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
    list_page(state, payload, 1).await
}

async fn list_page(state: &AppState, payload: &Value, page: usize) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    let mut owned = Vec::new();

    let application_ids = vc_core::application::owned_by(state.pool(), account).await?;
    let total = application_ids.len();
    let limit = developer::APPLICATIONS_PER_PAGE;
    let page = page.clamp(1, total.div_ceil(limit).max(1));
    for application_id in application_ids
        .into_iter()
        .skip((page - 1) * limit)
        .take(limit)
    {
        // The same render the HTTP reads answer with, so the menu and the page cannot
        // disagree about what an application is.
        let found = details(state.pool(), application_id)
            .await
            .map_err(vc_core::Error::from)?;

        if let Some(found) = found {
            owned.push(render(&found));
        }
    }

    Ok(ephemeral(vec![developer::applications_page(
        &owned,
        page,
        total,
        state.command_ids().await,
    )]))
}

/// A submitted settings form.
///
/// The values arrive as the modal's components, each keyed by the `custom_id` its input was
/// built with, and `body` is what the shared settings flow takes.
pub async fn modal(
    state: &AppState,
    screen: crate::custom_id::ui::developer::Screen,
    client_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let fields = submitted(payload);

    match screen {
        crate::custom_id::ui::developer::Screen::Edit => {
            // The id carries the field as well as the application — a form is for one of nine —
            // and the ownership check takes the `client_id` alone. The field is already in the
            // body: it is the `custom_id` the input was built with.
            let (client_id, _) = crate::custom_id::ui::developer::field_of(client_id);
            edit_form(state, client_id, body(fields), payload).await
        }
        // A screen with no form: reaching here means the id was built wrong.
        _ => Err(CommandError::Unknown),
    }
}

/// Acknowledge every settings submission before database work or a webhook handshake,
/// then replace the private loading response with the success or refusal screen.
pub async fn deferred_edit(
    state: &AppState,
    client_id: &str,
    payload: &Value,
) -> Result<Response, CommandError> {
    let field = |name| {
        payload
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CommandError::missing(&format!("interaction has no {name}")))
    };
    let interaction_id = field("id")?;
    let application_id = field("application_id")?.to_owned();
    let token = field("token")?.to_owned();

    // A deferred callback accepts only EPHEMERAL; components are supplied by the later edit.
    let acknowledgement = json!({"type": 5, "data": {"flags": crate::components::EPHEMERAL}});
    tokio::time::timeout(
        Duration::from_secs(2),
        state
            .discord()
            .create_interaction_response(interaction_id, &token, &acknowledgement),
    )
    .await
    .map_err(|_| CommandError::missing("the deferred response timed out"))??;

    // An explicit callback avoids racing an inline acknowledgement against the result edit.
    // No verification or database mutation starts if Discord did not acknowledge it.
    let state = state.clone();
    let client_id = client_id.to_owned();
    let payload = payload.clone();
    tokio::spawn(async move {
        let mut body = match modal(
            &state,
            crate::custom_id::ui::developer::Screen::Edit,
            &client_id,
            &payload,
        )
        .await
        {
            Ok(response) => response["data"].clone(),
            Err(error) => {
                tracing::warn!(?error, "application settings update failed");
                ephemeral(vec![developer::refusal(
                    message!("command.application.deferred_edit.001"),
                    Some(message!("command.application.deferred_edit.002")),
                )])
            }
        };
        // Privacy is fixed by the acknowledgement; the edit only enables components.
        body["flags"] = json!(crate::components::IS_COMPONENTS_V2);
        match tokio::time::timeout(
            Duration::from_secs(10),
            state
                .discord()
                .edit_original_interaction_response(&application_id, &token, &body),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "application settings response edit failed"),
            Err(_) => tracing::warn!("application settings response edit timed out"),
        }
    });

    Ok(StatusCode::ACCEPTED.into_response())
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
            message!("command.application.edit_form.001"),
            Some(message!("command.application.edit_form.002")),
        )));
    };

    let changes = match changes(&body) {
        Ok(changes) => changes,
        Err(refusal) => {
            return Ok(registered(developer::refusal(
                message!("command.application.edit_form.003"),
                refusal.description.as_deref(),
            )));
        }
    };

    // The application's own account, which is what the endpoint's token subject is, so the
    // handshake budget is the application's either way.
    let key = found.user_id.to_string();

    match apply(state, application_id, &key, &changes).await {
        Ok(()) => match details(state.pool(), application_id).await {
            Ok(Some(now)) => Ok(registered(developer::saved(
                application_screen(state, &now),
                if changes.rotate_client_secret {
                    message!("command.application.edit_form.004")
                } else {
                    message!("command.application.edit_form.005")
                },
            ))),
            _ => Ok(registered(developer::refusal(
                message!("command.application.edit_form.006"),
                None,
            ))),
        },
        Err(refusal) => Ok(registered(developer::refusal(
            message!("command.application.edit_form.007"),
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
        .and_then(crate::json_number::as_i64);

    match (component_type, screen) {
        // The list's menu. Its id says which screen the menu is on rather than what choosing
        // does, so the value that came back is the thing to act on, and it is a `client_id`.
        // A field's own menu: the id says which application and which field, and the values
        // that came back are the set the person chose. This comes first because the arm below
        // takes any string select and that one is the list's menu, not a field's.
        (Some(3), Screen::Edit) => {
            let (client_id, field) = crate::custom_id::ui::developer::field_of(&client_id);

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

            let response = edit_form(state, client_id, body, payload).await?;
            Ok(update(response["data"].clone()))
        }
        (Some(5), Screen::Connect) => {
            let bot = chosen(data)?
                .parse::<i64>()
                .map_err(|_| CommandError::Unknown)?;
            let Some((_, found)) = owned(state, &client_id, payload).await? else {
                return Ok(update(ephemeral(vec![developer::refusal(
                    message!("command.application.component.001"),
                    Some(message!("command.application.component.002")),
                )])));
            };
            if payload.get("guild_id").and_then(Value::as_str).is_none() {
                return Ok(update(ephemeral(vec![guild_required(state).await])));
            }
            if data
                .and_then(|data| data.get("resolved"))
                .and_then(|resolved| resolved.get("users"))
                .and_then(|users| users.get(bot.to_string()))
                .is_some_and(|user| user.get("bot") != Some(&Value::Bool(true)))
            {
                return Ok(update(ephemeral(vec![developer::refusal(
                    message!("command.application.component.003"),
                    Some(message!("command.application.component.004")),
                )])));
            }
            Ok(update(ephemeral(vec![developer::connect_confirmation(
                &client_id,
                found.discord_user_id,
                bot,
            )])))
        }
        (Some(2), Screen::ConfirmConnect) => {
            let (client_id, bot) = crate::custom_id::ui::developer::field_of(&client_id);
            Ok(update(ephemeral(vec![
                connect_bot(state, client_id, bot, payload).await?,
            ])))
        }
        (Some(2), Screen::RotateSecret) => {
            let (client_id, action) = crate::custom_id::ui::developer::field_of(&client_id);
            if action == "confirm" {
                let response = edit_form(
                    state,
                    client_id,
                    Map::from_iter([("client_secret".to_owned(), json!(true))]),
                    payload,
                )
                .await?;
                return Ok(update(response["data"].clone()));
            }
            if !action.is_empty() {
                return Err(CommandError::Unknown);
            }
            if owned(state, client_id, payload).await?.is_none() {
                return Ok(update(ephemeral(vec![developer::refusal(
                    message!("command.application.component.005"),
                    Some(message!("command.application.component.006")),
                )])));
            }
            Ok(update(ephemeral(vec![
                developer::rotate_secret_confirmation(client_id),
            ])))
        }
        (Some(2), Screen::Show) => Ok(update(show(state, &client_id, payload).await?)),
        // Only the application's list menu accepts an application id as its value.
        (Some(3), Screen::Back) => Ok(update(show(state, chosen(data)?, payload).await?)),
        // The list, from a screen that is not it: the one button left that goes anywhere.
        // 編集, which opens the form for the one field the id names.
        (Some(2), Screen::Edit) => {
            let (client_id, field) = crate::custom_id::ui::developer::field_of(&client_id);
            edit_field(state, client_id, field, payload).await
        }
        (Some(2), Screen::List) => {
            let (number, _) = crate::custom_id::ui::developer::field_of(&client_id);
            let page = if number.is_empty() {
                1
            } else {
                number.parse::<usize>().map_err(|_| CommandError::Unknown)?
            };
            Ok(update(list_page(state, payload, page).await?))
        }
        (Some(2), Screen::Back) => Ok(update(list(state, payload).await?)),
        _ => Err(CommandError::Unknown),
    }
}

/// The form for one field, which is what a text input needs.
///
/// A text input is a component only a modal may carry, so the six fields that are free text each
/// open one — and each form asks for exactly the field its button was beside, with what the field
/// holds now as the pre-filled value, so an untouched box comes back as what was there rather
/// than as a clearing.
async fn edit_field(
    state: &AppState,
    client_id: &str,
    field: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let Some((_, found)) = owned(state, client_id, payload).await? else {
        return Ok(message(ephemeral(vec![developer::refusal(
            message!("command.application.edit_field.001"),
            Some(message!("command.application.edit_field.002")),
        )])));
    };

    // `redirect_uris` is the one of these that is a list, and a paragraph input is the shape for
    // it: one URI per line, which is what `body` turns back into a list.
    let redirects = found.redirect_uris.join("\n");

    let (label, now, style, longest) = match field {
        "client_name" => (
            message!("command.application.edit_field.003"),
            found.client_name.as_deref(),
            crate::components::TextInputStyle::Short,
            Some(TextField::ClientName.max_chars() as u64),
        ),
        "redirect_uris" => (
            message!("command.application.edit_field.004"),
            Some(redirects.as_str()),
            crate::components::TextInputStyle::Paragraph,
            Some(vc_core::application::REDIRECT_URIS_MAX_CHARS as u64),
        ),
        "client_uri" => (
            message!("command.application.edit_field.005"),
            found.client_uri.as_deref(),
            crate::components::TextInputStyle::Short,
            Some(TextField::ClientUri.max_chars() as u64),
        ),
        "logo_uri" => (
            message!("command.application.edit_field.006"),
            found.logo_uri.as_deref(),
            crate::components::TextInputStyle::Short,
            Some(TextField::LogoUri.max_chars() as u64),
        ),
        "webhook_url" => (
            message!("common.webhookUrl"),
            found.webhook_url.as_deref(),
            crate::components::TextInputStyle::Short,
            Some(TextField::WebhookUrl.max_chars() as u64),
        ),
        "discord_support_server_invite_slug" => (
            message!("command.application.edit_field.007"),
            found.discord_support_server_invite_slug.as_deref(),
            crate::components::TextInputStyle::Short,
            Some(TextField::SupportInviteSlug.max_chars() as u64),
        ),
        // A field with no form, which means a button this module did not build.
        _ => return Err(CommandError::Unknown),
    };

    Ok(crate::components::modal(
        // The field is in the id, so the submission says which of the nine it is.
        &crate::custom_id::ui::developer::custom_id_for_field(
            crate::custom_id::ui::developer::Screen::Edit,
            &found.client_id,
            field,
        ),
        &format!(
            message!("command.application.edit_field.008"),
            label = label
        ),
        vec![crate::components::label(
            label,
            if style == crate::components::TextInputStyle::Paragraph {
                Some(message!("command.application.edit_field.009"))
            } else {
                None
            },
            crate::components::text_input(field, style, false, longest, now, None),
        )],
    ))
}

/// One field, as the request takes it.
///
/// Three of these fields are sets and one is a single value, and the difference is the
/// endpoint's: `grant_types`, `response_types` and `subscribed_events` are read as
/// arrays and `application_type` as a string. So a single choice is unwrapped here
/// rather than sent as an array the endpoint would refuse.
fn edited(field: &str, values: Vec<Value>) -> Option<(String, Value)> {
    match field {
        "application_type" => values
            .first()
            .map(|value| (field.to_owned(), value.clone())),
        "grant_types" | "response_types" | "subscribed_events" => {
            Some((field.to_owned(), Value::Array(values)))
        }
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
}
