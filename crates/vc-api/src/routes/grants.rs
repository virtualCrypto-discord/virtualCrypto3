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
//! Adding one asks Discord the question the consent screen asks — may this account
//! act for this guild — and writes the same grant the code flow would have. Taking
//! one away asks nothing of the guild, deliberately: an application's owner
//! narrowing what their application may do is not a decision the guild has to be
//! asked about.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

use vc_auth::AuthUser;

use crate::permissions::GuildAccess;
use crate::routes::connect::owned_by_client_id;
use crate::routes::oauth2_clients::{Refusal, internal, refusal, refused};
use crate::routes::v2::claims::format_timestamp;
use crate::state::AppState;

/// What the two writes take. The guild id is a string, like every Discord id this
/// service accepts, because one is a snowflake JSON's number cannot hold.
#[derive(Deserialize)]
pub struct Guild {
    pub guild_id: String,
}

/// `GET /applications/{id}/grants`
pub async fn index(
    State(state): State<AppState>,
    user: AuthUser,
    Path(client_id): Path<String>,
) -> Response {
    let application = match owned(&state, &user, &client_id).await {
        Ok((application, _)) => application,
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

/// `POST /applications/{id}/grants`: the guild's permission, written by its own
/// administrator.
///
/// The consent screen reaches the same write through a code exchange; this is the
/// same decision taken from the application's page, so the permission question is
/// the same one — [`crate::permissions::guild_access`] — and the guild is one the
/// caller may act for or the answer is a refusal.
pub async fn allow(
    State(state): State<AppState>,
    user: AuthUser,
    Path(client_id): Path<String>,
    Json(body): Json<Guild>,
) -> Response {
    let (application, account_id) = match owned(&state, &user, &client_id).await {
        Ok(found) => found,
        Err(refusal) => return refusal.response(),
    };

    let Ok(guild_id) = body.guild_id.parse::<i64>() else {
        return refused(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the guild id must be a Discord id, as a string",
        );
    };

    match crate::permissions::guild_access(&state, guild_id, account_id).await {
        GuildAccess::Permitted => {}
        GuildAccess::Unknown => {
            return refused(StatusCode::NOT_FOUND, "not_found", "no such guild");
        }
        GuildAccess::Denied => {
            return refused(StatusCode::FORBIDDEN, "forbidden", "permission_denied");
        }
    }

    let written = vc_core::grant::allow_in_guild(
        state.pool(),
        application,
        guild_id,
        &[vc_core::application::ISSUE],
        OffsetDateTime::now_utc(),
    )
    .await;

    match written {
        Ok(_) => (
            StatusCode::CREATED,
            Json(json!({
                "guild_id": guild_id.to_string(),
                "scopes": [vc_core::application::ISSUE],
            })),
        )
            .into_response(),
        Err(_) => internal("the grant could not be written").response(),
    }
}

/// `DELETE /applications/{id}/grants/{guild_id}`: the issuing permission, taken
/// back.
///
/// The grant stays and only its scope goes, so a guild token that was already
/// issued stops being able to issue and nothing else about the grant changes —
/// which is what [`vc_core::grant::disallow_in_guild`] does and why.
pub async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    Path((client_id, guild_id)): Path<(String, String)>,
) -> Response {
    let application = match owned(&state, &user, &client_id).await {
        Ok((application, _)) => application,
        Err(refusal) => return refusal.response(),
    };

    let Ok(guild_id) = guild_id.parse::<i64>() else {
        return refused(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the guild id must be a Discord id, as a string",
        );
    };

    match vc_core::grant::disallow_in_guild(
        state.pool(),
        application,
        guild_id,
        vc_core::application::ISSUE,
    )
    .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        // Nothing to take back is the answer the caller wanted, which is what RFC
        // 7009 says about revocation too.
        Ok(false) => StatusCode::NO_CONTENT.into_response(),
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
async fn owned(state: &AppState, user: &AuthUser, client_id: &str) -> Result<(i64, i32), Refusal> {
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
        Ok(Some((application, _))) => Ok((application, account_id)),
        Ok(None) => Err(
            refusal(StatusCode::NOT_FOUND, "not_found", "no such application")
                .as_ref()
                .clone(),
        ),
        Err(_) => Err(internal("the application could not be read")),
    }
}
