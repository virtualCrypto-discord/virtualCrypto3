//! `POST /oauth2/token`: the grants a client can use.
//!
//! The code exchange is here; the refresh and the two `client_credentials` shapes
//! are not yet, and answer `unsupported_grant_type` until they are — the same
//! answer a client gets for a grant that does not exist, which is the closest
//! thing to the truth that this endpoint can currently tell.

use axum::Json;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use time::OffsetDateTime;

use vc_core::grant::{EXPIRES_IN, ExchangeError, exchange_code, exchange_refresh_token};

use crate::state::AppState;

/// The body an authorization request arrives as.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TokenForm {
    pub grant_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code: Option<String>,
    pub refresh_token: Option<String>,
    #[allow(dead_code)]
    pub scope: Option<String>,
    #[allow(dead_code)]
    pub guild_id: Option<String>,
}

/// `POST /oauth2/token`.
pub async fn token(State(state): State<AppState>, Form(form): Form<TokenForm>) -> Response {
    match form.grant_type.as_deref() {
        None => error("invalid_request", "grant_type_parameter_missing"),
        Some("authorization_code") => exchange(&state, form).await,
        Some("refresh_token") => refresh(&state, form).await,
        Some(_) => unsupported(),
    }
}

/// `grant_type=authorization_code`.
async fn exchange(state: &AppState, form: TokenForm) -> Response {
    // Which parameter is missing is what the client is told, in the Elixir's
    // order.
    let Some(client_id) = form.client_id else {
        return error("invalid_request", "client_id");
    };
    let Some(redirect_uri) = form.redirect_uri else {
        return error("invalid_request", "redirect_uri");
    };
    let Some(code) = form.code else {
        return error("invalid_request", "code");
    };

    let exchanged = exchange_code(
        state.pool(),
        &client_id,
        &redirect_uri,
        &code,
        OffsetDateTime::now_utc(),
    )
    .await;

    match exchanged {
        Ok(exchanged) => {
            let mut body = json!({
                "access_token": exchanged.access_token,
                "token_type": "Bearer",
                "expires_in": exchanged.expires_in,
                "scopes": exchanged.scopes,
            });

            if let Some(refresh_token) = exchanged.refresh_token {
                body["refresh_token"] = json!(refresh_token);
            }

            Json(body).into_response()
        }
        Err(error) => refused(error),
    }
}

/// `grant_type=refresh_token`.
async fn refresh(state: &AppState, form: TokenForm) -> Response {
    let Some(refresh_token) = form.refresh_token else {
        return error("invalid_request", "refresh_token");
    };

    let refreshed =
        exchange_refresh_token(state.pool(), &refresh_token, OffsetDateTime::now_utc()).await;

    match refreshed {
        Ok(refreshed) => Json(json!({
            "access_token": refreshed.access_token,
            "token_type": "Bearer",
            // The literal the Elixir writes here, rather than the clock the
            // credentials path uses. Reproduced, not harmonised.
            "expires_in": EXPIRES_IN,
            "refresh_token": refreshed.refresh_token,
        }))
        .into_response(),
        Err(error) => refused(error),
    }
}

/// A grant this endpoint does not have. The Elixir's answer carries no
/// description for this one.
fn unsupported() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "unsupported_grant_type" })),
    )
        .into_response()
}

fn error(error: &str, description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

/// A refusal from the exchange.
///
/// Answered `400`, where the Elixir renders the same body with whatever status
/// the connection already had — which is `200`. RFC 6749 has `invalid_grant` at
/// `400`, and a client library that checks the status is the reason to follow it:
/// an error body under a success status is read as a success by exactly the
/// clients this endpoint exists for. The other grants in the Elixir do set `400`,
/// so this is the odd one out rather than the rule. Recorded in
/// docs/known-gaps.md.
fn refused(error: ExchangeError) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": error.error(),
            "error_description": error.description(),
        })),
    )
        .into_response()
}
