//! `/oauth2/clients/@me/grant-requests`: where an application asks a guild for a
//! permission, and reads back what the guild said.
//!
//! Not the Elixir's. There the only way an application got a grant was a person
//! redeeming an authorization code in a browser, so an application had no call of
//! its own that could ask. This is that ask, for the flows that have no browser:
//! the row it writes is the one `/grant list` shows the guild, and a guild's yes
//! is the grant the code flow would have written — after which
//! `client_credentials` with a `guild_id` answers with the token the endpoint
//! wants.
//!
//! The token is the application's own: `Kind::App` with `oauth2.register`, which
//! is exactly the token registration answered with. A user token cannot ask a
//! guild for a permission their application holds, because the caller's
//! application is not established by a user.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;

use vc_auth::AuthUser;

use crate::routes::oauth2_clients::{Refusal, internal, refusal, refused};
use crate::state::AppState;

/// What one request is, and what the application's own page will poll for.
#[derive(Deserialize)]
pub struct GrantRequest {
    pub guild_id: String,
}

/// `POST /oauth2/clients/@me/grant-requests`: ask a guild to let this
/// application issue from its pool.
///
/// The guild is named but never checked — it cannot be, because this service has
/// no proof the application belongs in any guild at all, and a wrong number in a
/// request is the guild's non-answer rather than this one's refusal. `guild_id` is
/// a string for the same reason every Discord id here is: a snowflake JSON's
/// number cannot hold.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<GrantRequest>,
) -> Response {
    let application = match own(&state, &user).await {
        Ok(application) => application,
        Err(refusal) => return refusal.response(),
    };

    let Ok(guild_id) = body.guild_id.parse::<i64>() else {
        return refused(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the guild id must be a Discord id, as a string",
        );
    };

    match vc_core::grant::request_grant(
        state.pool(),
        application,
        guild_id,
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": id.to_string(),
                "guild_id": guild_id.to_string(),
                "status": "pending",
            })),
        )
            .into_response(),
        Err(_) => internal("the request could not be written").response(),
    }
}

/// `GET /oauth2/clients/@me/grant-requests`: what this application asked for,
/// answered or not.
pub async fn index(State(state): State<AppState>, user: AuthUser) -> Response {
    let application = match own(&state, &user).await {
        Ok(application) => application,
        Err(refusal) => return refusal.response(),
    };

    match vc_core::grant::requests_of(state.pool(), application).await {
        Ok(requests) => Json(Value::Array(
            requests
                .into_iter()
                .map(|request| {
                    serde_json::json!({
                        "id": request.id.to_string(),
                        "guild_id": request.guild_id.to_string(),
                        "status": request.status,
                    })
                })
                .collect(),
        ))
        .into_response(),
        Err(_) => internal("the requests could not be read").response(),
    }
}

/// The application the token belongs to, which is also the whole of the
/// authorization question.
///
/// A user token asks with no application behind it — nothing says the caller *is*
/// the application a grant would be written for — so only the token issed at
/// registration time is let through. `oauth2.register` is required with it,
/// because asking a guild for a permission is application management, and that
/// token answered with that scope for exactly this.
async fn own(state: &AppState, user: &AuthUser) -> Result<i64, Refusal> {
    if user.kind != vc_auth::Kind::App {
        return Err(refusal(
            StatusCode::UNAUTHORIZED,
            "invalid_kind",
            "an application token is required",
        )
        .as_ref()
        .clone());
    }

    if !user.scopes.oauth2_register {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "oauth2.register is required",
        )
        .as_ref()
        .clone());
    }

    let Ok(subject) = i32::try_from(user.subject) else {
        return Err(internal("the token's subject is not an account id"));
    };

    match vc_core::user::application_id(state.pool(), subject).await {
        Ok(Some(application)) => Ok(application),
        Ok(None) => Err(internal("the application's account is gone")),
        Err(_) => Err(internal("the application could not be read")),
    }
}
