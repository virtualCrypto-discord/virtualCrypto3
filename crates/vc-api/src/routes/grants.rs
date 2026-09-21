//! `/applications/{id}/grants`: the guilds an application may issue in, as the
//! application's own page manages them.
//!
//! Not the Elixir's. There a grant only ever appeared as a side effect of redeeming
//! an authorization code, so an owner had no read that could list them and no call
//! that could take one away. The path and the ownership rule follow
//! `/applications/{id}/connect` next door: the `client_id` in the path, the caller's
//! own application, and a 404 for one that is not theirs so that somebody else's is
//! not confirmed to exist.
//!
//! Read and revoke only, on purpose: writing a grant is the guild's decision —
//! through an ask the application makes and the guild answers in Discord, or
//! through the consent screen — and a user token calling this endpoint is the
//! application's owner, not the guild. Taking one away asks nothing of the guild:
//! an application's owner narrowing what their application may do is not a
//! decision the guild has to be asked about.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use vc_auth::AuthUser;

use crate::routes::connect::owned_by_client_id;
use crate::routes::oauth2_clients::{Refusal, internal, refusal, refused};
use crate::routes::v2::claims::format_timestamp;
use crate::state::AppState;

/// `GET /applications/{id}/grants`
pub async fn index(
    State(state): State<AppState>,
    user: AuthUser,
    Path(client_id): Path<String>,
) -> Response {
    let application = match owned(&state, &user, &client_id).await {
        Ok(application) => application,
        Err(refusal) => return refusal.response(),
    };

    let grants = match vc_core::grant::grants_of(state.pool(), application).await {
        Ok(grants) => grants,
        Err(_) => return internal("the grants could not be read").response(),
    };

    let mut rendered: Vec<Value> = Vec::with_capacity(grants.len());

    for grant in grants {
        // The name is Discord's to know, not this service's, and a guild that cannot
        // be read is a row that is still real: the name is `null` rather than the
        // whole answer being an error.
        let name = state
            .discord()
            .get_guild(grant.guild_id)
            .await
            .ok()
            .flatten()
            .and_then(|guild| guild.get("name").and_then(Value::as_str).map(str::to_owned));

        rendered.push(json!({
            "guild_id": grant.guild_id.to_string(),
            "guild_name": name,
            "scopes": grant.scopes,
            "updated_at": format_timestamp(grant.updated_at),
        }));
    }

    Json(Value::Array(rendered)).into_response()
}

/// `DELETE /applications/{id}/grants/{guild_id}`: the issuing permission, taken
/// back.
///
/// The grant stays and only its scope goes, so a guild token that was already
/// issued stops being able to issue and nothing else about the grant changes —
/// which is what [`vc_core::grant::revoke_grant`] does from the page's side,
/// and why. The owner taking back is a decision the way the guild's yes is, so
/// it pings the same way: the guild and the scopes as they stand, empty now.
pub async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    Path((client_id, guild_id)): Path<(String, String)>,
) -> Response {
    let _ = match owned(&state, &user, &client_id).await {
        Ok(application) => application,
        Err(refusal) => return refusal.response(),
    };

    let Ok(guild_id) = guild_id.parse::<i64>() else {
        return refused(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the guild id must be a Discord id, as a string",
        );
    };

    // The page names the guild, and the application's own id comes from the
    // path: the revoke is the owner's, so the code is the `client_id` itself.
    match vc_core::grant::revoke_grant(state.pool(), &client_id, guild_id).await {
        Ok(Some(application_id)) => {
            state
                .notifier()
                .notify_grant_decided(application_id, guild_id);

            StatusCode::NO_CONTENT.into_response()
        }
        // Nothing to take back is the answer the caller wanted, which is what RFC
        // 7009 says about revocation too.
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => internal("the grant could not be written").response(),
    }
}

/// The caller's own application with this `client_id`, and the account it belongs
/// to.
///
/// Three answers, and the middle one is the point: a user token is required (an
/// application does not manage another's grants), the `oauth2.register` scope is
/// required because this is application management, and an application that is not
/// this account's is a **404** — the same nothing that a client id which does not
/// exist is, so that this endpoint cannot be used to ask which client ids are real.
async fn owned(state: &AppState, user: &AuthUser, client_id: &str) -> Result<i64, Refusal> {
    if user.kind != vc_auth::Kind::User {
        return Err(refusal(
            StatusCode::UNAUTHORIZED,
            "invalid_kind",
            "a user token is required",
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

    let Ok(account_id) = i32::try_from(user.subject) else {
        return Err(internal("the token's subject is not an account id"));
    };

    let owned = match vc_core::application::owned_by(state.pool(), account_id).await {
        Ok(owned) => owned,
        Err(_) => {
            return Err(internal(
                "the applications an account owns could not be read",
            ));
        }
    };

    match owned_by_client_id(state.pool(), &owned, client_id).await {
        Ok(Some((application, _))) => Ok(application),
        Ok(None) => Err(
            refusal(StatusCode::NOT_FOUND, "not_found", "no such application")
                .as_ref()
                .clone(),
        ),
        Err(_) => Err(internal("the application could not be read")),
    }
}
