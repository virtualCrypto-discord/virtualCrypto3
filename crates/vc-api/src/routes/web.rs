//! The pages a browser visits, as opposed to the API an SPA calls.
//!
//! These are the two ends of a Discord login: sending the browser to Discord
//! with a state to check the answer against, and ending the session afterwards.

use axum::extract::{Query, State};
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use uuid::Uuid;

use crate::session::{LoginAttempt, Session, clear_cookie, set_cookie};
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
