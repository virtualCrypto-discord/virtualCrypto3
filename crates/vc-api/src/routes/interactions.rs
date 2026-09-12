use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::command::CommandError;
use crate::state::AppState;

/// `POST /api/integrations/discord/interactions`
///
/// Mirrors `InteractionsController.index/2`: the signature is verified before
/// anything else, and only then does the body's `type` select a handler. The
/// request signature is the whole of the authentication — there is no token.
pub async fn index(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let signature = single_header(&headers, "x-signature-ed25519");
    let timestamp = single_header(&headers, "x-signature-timestamp");

    let (Some(signature), Some(timestamp)) = (signature, timestamp) else {
        return text(StatusCode::UNAUTHORIZED, "invalid request signature");
    };

    if !crate::discord::verify_signature(state.discord_public_key(), signature, timestamp, &body) {
        return text(StatusCode::UNAUTHORIZED, "invalid request signature");
    }

    // An unparsable body is a `Type Not Found` rather than a signature failure:
    // the signature already passed, so the request is genuinely ours.
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return text(StatusCode::BAD_REQUEST, "Type Not Found");
    };

    match payload.get("type").and_then(Value::as_i64) {
        // 1: PING, answered with a PONG.
        Some(1) => (StatusCode::OK, Json(json!({ "type": 1 }))).into_response(),
        Some(2) => command(&state, &payload).await,
        // 3 message components, 4 autocomplete and 5 modals are the remaining
        // work; see docs/known-gaps.md.
        Some(kind @ 3..=5) => text(
            StatusCode::NOT_IMPLEMENTED,
            &format!("interaction type {kind} is not implemented yet"),
        ),
        _ => text(StatusCode::BAD_REQUEST, "Type Not Found"),
    }
}

/// `verified/2` for `type` 2: the command name picks a handler, and a payload
/// without one falls through to `Type Not Found`.
async fn command(state: &AppState, payload: &Value) -> Response {
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

    match crate::command::handle(state, name, &options, payload).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
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
