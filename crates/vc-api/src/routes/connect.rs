//! `POST /applications/{id}/connect`: binding an application to the bot that speaks
//! for it.
//!
//! An application's account is created with no `discord_id`, and this is what gives it
//! one. The proof is not in the request: the guild's integrations are read from
//! Discord, the one whose `application.bot.id` is the submitted bot is found, and only
//! then must its `application.description` contain the application's client id. So the
//! operator writes the id into the bot's integration description and this reads it
//! back — which is why a request cannot simply assert ownership.
//!
//! **Two of the answers here are decisions rather than ports.** The Elixir is a
//! LiveView: it knows the application from the session, and its answers are sentences
//! for a person reading a page. So the caller is checked against the applications the
//! account owns — the same read `GET /oauth2/clients/@me` answers with — and the
//! failures are the `error` / `error_description` shape used next door rather than six
//! sentences.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::PgPool;

use vc_auth::AuthUser;

use crate::routes::oauth2_clients::{Details, Refusal, details, internal, refusal, refused};
use crate::state::AppState;

/// What the connect form sends.
///
/// Both ids are strings, and that is not a stylistic choice: a Discord id is a snowflake
/// in the region of 10^18, and JSON's number is a double, which stops counting exactly
/// at 2^53. A client that sends one as a number has already lost digits before this
/// service sees it, so both arrive as text and are parsed here, where an `i64` is exact.
///
/// The Elixir took them from a form, which is text for the same reason.
#[derive(Deserialize)]
pub struct Connect {
    pub bot_id: String,
    pub guild_id: String,
}

/// A field of one of Discord's objects, or `"unknown"` — the answers here name things,
/// and a name that is missing should not be a panic.
fn text<'a>(object: &'a Map<String, Value>, key: &str) -> &'a str {
    object.get(key).and_then(Value::as_str).unwrap_or("unknown")
}

/// The application the caller owns whose `client_id` this is, or nothing.
///
/// Ownership is decided first and by id, so a client id that belongs to somebody else
/// is indistinguishable from one that does not exist. That is the whole reason the
/// answer to both is a 404: this endpoint must not confirm which client ids are real.
/// The caller's own application that this `client_id` names, with its id.
///
/// The id comes back too because two callers want two halves of the same answer: a screen
/// wants the application, and an edit wants something to write to. Asking twice would be
/// reading it twice.
pub(crate) async fn owned_by_client_id(
    pool: &PgPool,
    owned: &[i64],
    client_id: &str,
) -> Result<Option<(i64, Details)>, sqlx::Error> {
    for id in owned {
        if let Some(found) = details(pool, *id).await?
            && found.client_id == client_id
        {
            return Ok(Some((*id, found)));
        }
    }

    Ok(None)
}

pub async fn connect(
    State(state): State<AppState>,
    user: AuthUser,
    Path(client_id): Path<String>,
    Json(body): Json<Connect>,
) -> Response {
    if user.kind != vc_auth::Kind::User {
        return refused(
            StatusCode::UNAUTHORIZED,
            "invalid_kind",
            "a user token is required",
        );
    }

    let Ok(subject) = i32::try_from(user.subject) else {
        return internal("the subject is out of range for an account id").response();
    };

    // Parsed here, once, and never elsewhere: a Discord id that does not fit an `i64`
    // is not a Discord id, and turning one into a guild number some other way would
    // answer about the wrong guild rather than about the request.
    let (Ok(guild_id), Ok(bot_id)) = (body.guild_id.parse::<i64>(), body.bot_id.parse::<i64>())
    else {
        return refused(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "both ids must be Discord ids, as strings",
        );
    };

    // The owner and nobody else, and 404 rather than 403 so that somebody else's
    // application is not confirmed to exist.
    //
    // The path carries the client id, because that is what `/applications/:id` means:
    // the old site's list links `"/applications/" ++ application.client_id`, and the
    // connect route is the same `:id`. The numeric id is this service's own, is not
    // handed out by any read, and so is not something a caller could name.
    let owned = match vc_core::application::owned_by(state.pool(), subject).await {
        Ok(owned) => owned,
        Err(_) => return internal("the applications an account owns could not be read").response(),
    };

    // The application's own account, which is what the write at the end gives a
    // Discord id to, and the client id the description must contain.
    let found = match owned_by_client_id(state.pool(), &owned, &client_id).await {
        Ok(Some((_, found))) => found,
        Ok(None) => {
            return refused(StatusCode::NOT_FOUND, "not_found", "no such application");
        }
        Err(_) => return internal("the application could not be read").response(),
    };

    match connect_application(&state, &found, &body.bot_id, bot_id, guild_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(refusal) => refusal.response(),
    }
}

/// Binding an application to the bot that speaks for it, from where both surfaces can meet.
///
/// The endpoint establishes its caller with a token and finds the application among the ones
/// that account owns. A command in a guild has the interaction for the caller and the guild's
/// id already, so it makes the same ownership check itself. What is left is one piece of work
/// either way — read the guild's integrations from Discord, find the one whose bot this is,
/// insist that its description names the application, and give the application's account that
/// bot — and it is here once, because the proof does not care which surface asked.
pub async fn connect_application(
    state: &AppState,
    application: &Details,
    bot: &str,
    bot_id: i64,
    guild_id: i64,
) -> Result<(), Box<Refusal>> {
    let (status, integrations) = match state
        .discord()
        .get_guild_integrations_with_status(guild_id)
        .await
    {
        Ok(answer) => answer,
        Err(_) => {
            return Err(Box::new(internal(
                "the guild's integrations could not be read",
            )));
        }
    };

    // 403 is two different things and the operator acts on them differently, so the
    // guild is read to say which one it is.
    if status == 403 {
        return match state.discord().get_guild_with_status(guild_id).await {
            Ok((403, _)) => Err(refusal(
                StatusCode::FORBIDDEN,
                "not_installed",
                "VirtualCrypto is not in that server",
            )),
            Ok((200, guild)) => Err(refusal(
                StatusCode::FORBIDDEN,
                "insufficient_permissions",
                format!(
                    "VirtualCrypto is in {} but does not have Manage Server there",
                    text(&guild, "name")
                ),
            )),
            _ => Err(Box::new(internal(
                "the guild could not be read after a refused integrations call",
            ))),
        };
    }

    if status == 404 {
        return Err(refusal(StatusCode::NOT_FOUND, "not_found", "no such guild"));
    }

    if status != 200 {
        return Err(refusal(
            StatusCode::BAD_GATEWAY,
            "discord_error",
            format!("fetching integrations failed with {status}"),
        ));
    }

    // The bot's own id is a string in Discord's answer, as every Discord id is, so
    // this compares text rather than parsing.
    let integration = integrations.iter().find(|integration| {
        integration
            .get("application")
            .and_then(|application| application.get("bot"))
            .and_then(|bot| bot.get("id"))
            .and_then(Value::as_str)
            == Some(bot)
    });

    let Some(integration) = integration else {
        // Why there is no such integration, which needs the id looked up and then the
        // guild, because the answer names it.
        return match state.discord().get_user_with_status(bot_id).await {
            Ok((200, user)) if user.get("bot") == Some(&Value::Bool(true)) => {
                let guild = state.discord().get_guild_with_status(guild_id).await;

                match guild {
                    Ok((200, guild)) => Err(refusal(
                        StatusCode::NOT_FOUND,
                        "invalid_bot",
                        format!(
                            "{} is not in {}",
                            text(&user, "username"),
                            text(&guild, "name")
                        ),
                    )),
                    _ => Err(Box::new(internal(
                        "the guild could not be read while explaining a missing bot",
                    ))),
                }
            }
            Ok((200, user)) => Err(refusal(
                StatusCode::BAD_REQUEST,
                "invalid_bot",
                format!("{} is not a bot", text(&user, "username")),
            )),
            Ok((404, _)) => Err(refusal(
                StatusCode::NOT_FOUND,
                "invalid_bot",
                "no such user id",
            )),
            _ => Err(Box::new(internal("the bot id could not be looked up"))),
        };
    };

    let describes = integration
        .get("application")
        .and_then(|application| application.get("description"))
        .and_then(Value::as_str)
        .is_some_and(|description| description.contains(&application.client_id));

    if !describes {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "invalid_description",
            format!(
                "the integration's description does not contain this application's client id (bot: {})",
                integration
                    .get("application")
                    .and_then(|application| application.get("bot"))
                    .and_then(Value::as_object)
                    .map_or("unknown", |bot| text(bot, "username"))
            ),
        ));
    }

    match vc_core::user::bind_bot(state.pool(), application.user_id, bot_id).await {
        Ok(()) => Ok(()),
        // The one failure with a message of its own in the Elixir, because one bot
        // belongs to one application.
        Err(vc_core::user::BindError::Taken) => Err(refusal(
            StatusCode::CONFLICT,
            "already_connected",
            "that bot already belongs to another application",
        )),
        Err(vc_core::user::BindError::Database(_)) => Err(Box::new(internal(
            "the bot could not be bound to the application",
        ))),
    }
}
