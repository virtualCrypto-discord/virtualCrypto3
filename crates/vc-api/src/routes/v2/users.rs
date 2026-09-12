use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};
use time::Duration;
use vc_auth::AuthUser;
use vc_core::model::{DiscordAuth, utc_now};

use crate::discord::filter_profile;
use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/v2/users/@me`
///
/// Mirrors `VirtualCryptoWeb.Api.V1V2.UserController.me/2`: resolve the local
/// user, take its stored Discord authorization (refreshing it first when it is
/// near expiry), fetch the Discord profile, and return
/// `{"id": "<local user id>", "discord": <filtered profile>}`.
pub async fn me(State(state): State<AppState>, user: AuthUser) -> Result<Json<Value>, ApiError> {
    let local_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    let account = vc_core::user::find_by_id(state.pool(), local_id)
        .await?
        .ok_or(vc_core::Error::UserNotFound(user.subject))?;

    let discord_id = account
        .discord_id
        .ok_or_else(|| ApiError::Internal(format!("user {local_id} has no discord id")))?;

    let authorization = vc_core::user::find_discord_auth(state.pool(), discord_id)
        .await?
        .ok_or(vc_core::Error::DiscordAuthNotFound(discord_id))?;

    let token = resolve_token(&state, discord_id, &authorization).await?;

    let profile = state.discord().get_user_info(&token).await?;

    Ok(Json(json!({
        "id": account.id.to_string(),
        "discord": filter_profile(profile),
    })))
}

/// Reproduces `DiscordAuth.refresh_user/1` followed by the controller's use of
/// the returned struct: refresh inside the final 15 minutes of the seven-day
/// lifetime, store the new authorization, and use the refreshed token.
async fn resolve_token(
    state: &AppState,
    discord_id: i64,
    authorization: &DiscordAuth,
) -> Result<String, ApiError> {
    if !authorization.needs_refresh(utc_now()) {
        return authorization
            .token
            .clone()
            .ok_or_else(|| ApiError::Internal(format!("discord user {discord_id} has no token")));
    }

    // A refresh failure returns `:error` in Elixir, which the controller then
    // dereferences, producing a 500.
    let refresh_token = authorization.refresh_token.clone().ok_or_else(|| {
        ApiError::Internal(format!("discord user {discord_id} has no refresh token"))
    })?;

    let refreshed = state.discord().refresh_token(&refresh_token).await?;

    let expires = utc_now() + Duration::seconds(refreshed.expires_in);
    vc_core::user::update_discord_token(
        state.pool(),
        discord_id,
        &refreshed.token,
        refreshed.refresh_token.as_deref(),
        expires,
    )
    .await?;

    Ok(refreshed.token)
}
