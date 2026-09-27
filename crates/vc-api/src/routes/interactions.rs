use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::command::CommandError;
use crate::state::AppState;

/// `POST /api/integrations/discord/interactions`
///
/// Mirrors `InteractionsController.index/2`: the signature is verified before
/// anything else, and only then does the body's `type` select a handler. The
/// request signature is the whole of the authentication — there is no token.
pub async fn index(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let received_at = Instant::now();
    let signature = single_header(&headers, "x-signature-ed25519");
    let timestamp = single_header(&headers, "x-signature-timestamp");

    let (Some(signature), Some(timestamp)) = (signature, timestamp) else {
        return refused(
            &state,
            "the signature headers were not both present",
            "invalid request signature",
        )
        .await;
    };

    if !crate::discord::verify_signature(state.discord_public_key(), signature, timestamp, &body) {
        return refused(
            &state,
            "the signature is not Discord's",
            "invalid request signature",
        )
        .await;
    }

    // Receipts are purged after 24 hours. Accept signed requests for only five
    // minutes, with 30 seconds of forward clock skew, so deleting a receipt
    // cannot make its old signed request executable again.
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if !timestamp
        .parse::<i64>()
        .is_ok_and(|signed_at| (now - 300..=now + 30).contains(&signed_at))
    {
        return refused(
            &state,
            "the signature's timestamp is outside the window",
            "expired request signature",
        )
        .await;
    }

    // An unparsable body is a `Type Not Found` rather than a signature failure:
    // the signature already passed, so the request is genuinely ours.
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return text(StatusCode::BAD_REQUEST, "Type Not Found");
    };

    // A loose per-user allowance, held against the authenticated Discord user.
    if let Some(user) = crate::command::get_user(&payload) {
        state.monitor().watch(&format!("discord:{user}")).await;
        if let Err(remaining) = state.limiter().allow(&format!("discord:{user}")) {
            state
                .monitor()
                .observe(
                    crate::security::Signal::RateLimited,
                    &format!("discord:{user}"),
                    "the loose per-caller limit refused a request",
                )
                .await;
            let mut response = text(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests");
            response
                .headers_mut()
                .insert(RETRY_AFTER, crate::rate_limit::retry_after(remaining));
            return response;
        }
    }

    if matches!(
        payload.get("type").and_then(crate::json_number::as_i64),
        Some(2 | 3 | 5)
    ) {
        return super::interaction_receipts::run(state, payload, received_at).await;
    }
    dispatch(&state, &payload, received_at).await
}

pub(super) async fn dispatch(state: &AppState, payload: &Value, received_at: Instant) -> Response {
    match payload.get("type").and_then(crate::json_number::as_i64) {
        // 1: PING, answered with a PONG.
        Some(1) => (StatusCode::OK, Json(json!({ "type": 1 }))).into_response(),
        Some(2) => command(state, payload, received_at).await,
        Some(3) => component(state, payload).await,
        Some(4) => autocomplete(state, payload).await,
        Some(5) => modal(state, payload).await,
        _ => text(StatusCode::BAD_REQUEST, "Type Not Found"),
    }
}

/// An interaction Discord would not have sent: no signature, one that does not
/// verify, or a timestamp outside the window. The caller keeps the answer it
/// always had — which part failed is this service's to know, and the repetition
/// of any of them is what is worth saying to an operator.
async fn refused(state: &AppState, why: &str, body: &'static str) -> Response {
    state
        .monitor()
        .observe(
            crate::security::Signal::InteractionSignature,
            "interaction-signature",
            why,
        )
        .await;

    text(StatusCode::UNAUTHORIZED, body)
}

/// `verified/2` for `type` 4: the option being typed, and the path that says
/// which suggestions it wants.
async fn autocomplete(state: &AppState, payload: &Value) -> Response {
    let data = payload.get("data");

    let Some(name) = data
        .and_then(|data| data.get("name"))
        .and_then(Value::as_str)
    else {
        return text(StatusCode::BAD_REQUEST, "Type Not Found");
    };

    let options = data
        .and_then(|data| data.get("options"))
        .and_then(Value::as_array);

    // A subcommand's options sit one level down, and the focused one is the
    // option Discord says the user is typing in.
    let (path, focused) = match options.and_then(|options| options.first()) {
        Some(option) if option.get("type").and_then(crate::json_number::as_i64) == Some(1) => {
            let subcommand = option
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();

            (
                vec![name, subcommand],
                focused_option(option.get("options").and_then(Value::as_array)),
            )
        }
        _ => (vec![name], focused_option(options)),
    };

    let Some(focused) = focused else {
        return text(StatusCode::BAD_REQUEST, "Type Not Found");
    };

    match crate::command::autocomplete::handle(state, &path, focused, payload).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
        Err(CommandError::Internal(error)) => error.into_response(),
    }
}

fn focused_option(options: Option<&Vec<Value>>) -> Option<&Value> {
    options?
        .iter()
        .find(|option| option.get("focused").and_then(Value::as_bool) == Some(true))
}

/// `verified/2` for `type` 5: a modal submission, whose `custom_id` says which
/// one was filled in.
async fn modal(state: &AppState, payload: &Value) -> Response {
    let Some(custom_id) = payload
        .get("data")
        .and_then(|data| data.get("custom_id"))
        .and_then(Value::as_str)
    else {
        return text(StatusCode::BAD_REQUEST, "Type Not Found");
    };

    // The developer screens have a head byte of their own, so trying them first cannot
    // steal another space's id — which it did, before they had one.
    if let Ok((screen, client_id)) =
        crate::custom_id::ui::developer::parse(&crate::custom_id::parse(custom_id))
    {
        if matches!(screen, crate::custom_id::ui::developer::Screen::Edit) {
            return match crate::command::application::deferred_edit(state, &client_id, payload)
                .await
            {
                Ok(response) => response,
                Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
                Err(CommandError::Internal(error)) => error.into_response(),
            };
        }
        return match crate::command::application::modal(state, screen, &client_id, payload).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
            Err(CommandError::Internal(error)) => error.into_response(),
        };
    }

    let path = crate::custom_id::ui::modal::parse(&crate::custom_id::parse(custom_id));

    match path {
        Ok((["delete", "confirm"], _)) => management_response(
            crate::command::response::run(state, payload, false, |state, payload| async move {
                crate::command::delete::confirm(&state, &payload).await
            })
            .await,
        ),
        _ => text(StatusCode::BAD_REQUEST, "Type Not Found"),
    }
}

/// `verified/2` for `type` 3: a message component. `component_type` picks the
/// handler and the `custom_id` carries the state it acts on.
async fn component(state: &AppState, payload: &Value) -> Response {
    let data = payload.get("data");

    let custom_id = data
        .and_then(|data| data.get("custom_id"))
        .and_then(Value::as_str);
    let component_type = data
        .and_then(|data| data.get("component_type"))
        .and_then(crate::json_number::as_i64);

    // The developer screens have a head byte of their own, so trying them first cannot read a
    // button that belongs to another screen — which is what their ids are for.
    if let Some(custom_id) = custom_id
        && let Ok((screen, _)) =
            crate::custom_id::ui::developer::parse(&crate::custom_id::parse(custom_id))
    {
        use crate::custom_id::ui::developer::Screen;
        if matches!(
            (component_type, screen),
            (Some(3), Screen::Edit) | (Some(5), Screen::Connect)
        ) {
            let custom_id = custom_id.to_owned();
            return management_response(
                crate::command::response::run(state, payload, true, |state, payload| async move {
                    crate::command::application::component(&state, &custom_id, &payload).await
                })
                .await,
            );
        }
        return match crate::command::application::component(state, custom_id, payload).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
            Err(CommandError::Internal(error)) => error.into_response(),
        };
    }

    // The contract buttons carry a head byte of their own, for the developer
    // screens' reason: their ids name a contract, and a parser that only accepts
    // its own head refuses the other spaces' bytes rather than reading them.
    if let Some(custom_id) = custom_id
        && crate::custom_id::ui::contract::parse(&crate::custom_id::parse(custom_id)).is_ok()
    {
        return match crate::command::contract::respond(state, custom_id, payload).await {
            Ok(response) => response,
            Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
            Err(CommandError::Internal(error)) => error.into_response(),
        };
    }

    // And `/help`'s menu, which needs its own head for the same reason: the arm
    // below hands every string select to the claim list.
    if let Some(custom_id) = custom_id
        && crate::custom_id::ui::help::parse(&crate::custom_id::parse(custom_id)).is_ok()
    {
        return match crate::command::help::component(state, custom_id, payload).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
            Err(CommandError::Internal(error)) => error.into_response(),
        };
    }

    // The grant buttons carry a head byte of their own, for the same reason: each
    // names the application whose permission it takes back.
    if let Some(custom_id) = custom_id
        && let Ok(pressed) = crate::custom_id::ui::grant::parse(&crate::custom_id::parse(custom_id))
    {
        use crate::custom_id::ui::grant::Pressed;
        if matches!(pressed, Pressed::Confirmed(_) | Pressed::RevokeOne(_)) {
            let custom_id = custom_id.to_owned();
            return management_response(
                crate::command::response::run(state, payload, true, |state, payload| async move {
                    crate::command::grant::component(&state, &custom_id, &payload).await
                })
                .await,
            );
        }
        return match crate::command::grant::component(state, custom_id, payload).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
            Err(CommandError::Internal(error)) => error.into_response(),
        };
    }

    // And the balance list's arrows, which carry a page and nothing else.
    if let Some(custom_id) = custom_id
        && crate::custom_id::ui::bal::parse(&crate::custom_id::parse(custom_id)).is_ok()
    {
        return match crate::command::bal::component(state, custom_id, payload).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
            Err(CommandError::Internal(error)) => error.into_response(),
        };
    }

    // The mutes screen's arrows and its one button per row, which name the currency or the
    // person a press puts back.
    if let Some(custom_id) = custom_id
        && let Ok(pressed) = crate::custom_id::ui::mute::parse(&crate::custom_id::parse(custom_id))
    {
        use crate::custom_id::ui::mute::Pressed;
        if matches!(pressed, Pressed::Currency(_) | Pressed::User(_)) {
            let custom_id = custom_id.to_owned();
            return management_response(
                crate::command::response::run(state, payload, true, |state, payload| async move {
                    crate::command::mute::component(&state, &custom_id, &payload).await
                })
                .await,
            );
        }
        return match crate::command::mute::component(state, custom_id, payload).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
            Err(CommandError::Internal(error)) => error.into_response(),
        };
    }

    // And the history screens' arrows, which carry a ledger, a page and what it is narrowed to.
    if let Some(custom_id) = custom_id
        && crate::custom_id::ui::history::parse(&crate::custom_id::parse(custom_id)).is_ok()
    {
        return match crate::command::history::component(state, custom_id, payload).await {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
            Err(CommandError::Internal(error)) => error.into_response(),
        };
    }

    match (component_type, custom_id) {
        (Some(2), Some(custom_id)) => {
            match crate::command::claim::button::handle(state, custom_id, payload).await {
                Ok(response) => response,
                Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
                Err(CommandError::Internal(error)) => error.into_response(),
            }
        }
        _ => text(StatusCode::BAD_REQUEST, "Type Not Found"),
    }
}

/// `verified/2` for `type` 2: the command name picks a handler, and a payload
/// without one falls through to `Type Not Found`.
async fn command(state: &AppState, payload: &Value, received_at: Instant) -> Response {
    let data = payload.get("data");

    let Some(name) = data
        .and_then(|data| data.get("name"))
        .and_then(Value::as_str)
    else {
        return text(StatusCode::BAD_REQUEST, "Type Not Found");
    };

    let options = data
        .and_then(|data| data.get("options"))
        .and_then(Value::as_array)
        .map(|options| crate::command::parse_options(options))
        .unwrap_or_default();

    let mutating_claim = name == "claim"
        && matches!(
            options.get("subcommand").and_then(Value::as_str),
            Some("make" | "approve" | "deny" | "cancel")
        );
    if matches!(name, "pay" | "issue") || mutating_claim {
        let response = match name {
            "pay" => crate::command::pay::respond(state, &options, payload, received_at).await,
            "issue" => crate::command::issue::respond(state, &options, payload).await,
            _ => crate::command::claim::respond(state, &options, payload).await,
        };
        return match response {
            Ok(response) => response,
            Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
            Err(CommandError::Internal(error)) => error.into_response(),
        };
    }

    let subcommand = options.get("subcommand").and_then(Value::as_str);
    let management = match name {
        "create" => true,
        "pat" => matches!(subcommand, Some("create" | "revoke")),
        "mute" | "unmute" => matches!(subcommand, Some("currency" | "user")),
        "application" => subcommand == Some("register"),
        _ => false,
    };
    if management {
        let name = name.to_owned();
        return management_response(
            crate::command::response::run(state, payload, false, |state, payload| async move {
                crate::command::handle(&state, &name, &options, &payload).await
            })
            .await,
        );
    }

    match crate::command::handle(state, name, &options, payload).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
        Err(CommandError::Internal(error)) => error.into_response(),
    }
}

fn management_response(result: Result<Response, CommandError>) -> Response {
    match result {
        Ok(response) => response,
        Err(CommandError::Unknown) => text(StatusCode::BAD_REQUEST, "Type Not Found"),
        Err(CommandError::Internal(error)) => error.into_response(),
    }
}

/// A header that must appear exactly once; two values make it unusable, which is
/// what `Plug.Conn.get_req_header/2` matching a single-element list does.
fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;

    if values.next().is_some() {
        return None;
    }

    value.to_str().ok()
}

fn text(status: StatusCode, body: &str) -> Response {
    (
        status,
        [(CONTENT_TYPE, HeaderValue::from_static("text/plain"))],
        body.to_string(),
    )
        .into_response()
}
