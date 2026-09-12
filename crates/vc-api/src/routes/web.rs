//! The pages a browser visits, as opposed to the API an SPA calls.
//!
//! These are the two ends of a Discord login: sending the browser to Discord
//! with a state to check the answer against, and ending the session afterwards.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::Value;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use crate::session::{self, LoginAttempt, Session, clear_cookie, set_cookie};
use crate::state::AppState;

/// Where to come back to once Discord has answered.
#[derive(Deserialize)]
pub struct LoginQuery {
    /// The old site read this from the request that had been refused. An SPA
    /// cannot: a 401 from an API call does not know which page asked, so the
    /// page says where it wants to return.
    #[serde(default = "site_root")]
    #[serde(rename = "continue")]
    continue_to: String,
}

fn site_root() -> String {
    "/".to_owned()
}

/// Send the browser to Discord, remembering a state to check the answer against.
pub async fn login(State(state): State<AppState>, Query(query): Query<LoginQuery>) -> Response {
    let attempt = LoginAttempt {
        state: Uuid::new_v4().to_string(),
        continue_to: query.continue_to,
    };

    let session = Session::awaiting_discord(attempt.clone());

    match set_cookie(&session, state.session_secret(), state.secure_cookies()) {
        Ok(cookie) => {
            let url = state.discord().authorize_url(&attempt.state);
            ([(SET_COOKIE, cookie)], Redirect::to(&url)).into_response()
        }
        // Without a session there is no state to check Discord's answer
        // against, so sending the browser there would only invite a callback
        // that has to be refused.
        Err(_) => Redirect::to("/").into_response(),
    }
}

/// End the session.
///
/// A signed cookie cannot be un-signed, only replaced by one that has already
/// expired. The attributes have to match the cookie being replaced, or the
/// browser keeps the original.
pub async fn logout(State(state): State<AppState>) -> Response {
    (
        [(SET_COOKIE, clear_cookie(state.secure_cookies()))],
        Redirect::to("/"),
    )
        .into_response()
}

/// The answer Discord sends back.
#[derive(Deserialize)]
pub struct CallbackQuery {
    state: String,
    code: String,
}

/// Refuse a callback, forgetting the login that was in flight.
///
/// The state is what makes an answer checkable at all: without a session that
/// remembers what was asked for, an answer cannot be told apart from one
/// somebody else solicited.
fn refuse(state: &AppState, why: &str) -> Response {
    tracing::info!(why, "refusing a Discord callback");

    (
        [(SET_COOKIE, clear_cookie(state.secure_cookies()))],
        Redirect::to("/"),
    )
        .into_response()
}

/// Discord's answer to a login: check the state, exchange the code, and put the
/// account in the session.
///
/// No token is issued here, unlike the old site, which handed one back in
/// response headers and a rendered page. Issuing one only for the SPA to
/// immediately ask for another would write a `user_access_tokens` row nobody
/// will ever use; `POST /token` is where a browser gets one.
pub async fn discord_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(session) = session::from_headers(&headers, state.session_secret()) else {
        return refuse(&state, "no session");
    };

    let Some(attempt) = session.discord_oauth2 else {
        return refuse(&state, "no login in flight");
    };

    if attempt.state != query.state {
        return refuse(&state, "the state does not match the one we sent");
    }

    let Ok(token) = state.discord().exchange_code(&query.code).await else {
        return refuse(&state, "the code could not be exchanged");
    };

    let Ok(profile) = state.discord().get_user_info(&token.token).await else {
        return refuse(&state, "the profile could not be read");
    };

    let Some(discord_user_id) = profile
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse::<i64>().ok())
    else {
        return refuse(&state, "the profile has no usable id");
    };

    let now = OffsetDateTime::now_utc();
    let expires =
        PrimitiveDateTime::new(now.date(), now.time()) + Duration::seconds(token.expires_in);

    let Ok(user) = vc_core::user::insert_user(
        state.pool(),
        discord_user_id,
        &token.token,
        token.refresh_token.as_deref(),
        expires,
    )
    .await
    else {
        return refuse(&state, "the authorization could not be recorded");
    };

    match set_cookie(
        &Session::logged_in(i64::from(user.id)),
        state.session_secret(),
        state.secure_cookies(),
    ) {
        Ok(cookie) => ([(SET_COOKIE, cookie)], Redirect::to(&attempt.continue_to)).into_response(),
        Err(_) => refuse(&state, "the session could not be signed"),
    }
}
