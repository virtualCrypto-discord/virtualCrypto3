//! The Discord authorization an account holds, and using it.
//!
//! Shared by the v2 user endpoint and by application registration, which need the
//! same thing from it: a token to ask Discord with. Both use the stored expiry to
//! decide when to refresh, then save the replacement token and its expiry here.

use time::Duration;

use vc_core::model::{DiscordAuth, utc_now};

use crate::error::ApiError;
use crate::state::AppState;

/// Refresh inside the final 15 minutes of the stored lifetime, store the new
/// authorization and expiry, and use the refreshed token. Authorizations with no
/// stored expiry use the legacy seven-day estimate in `DiscordAuth::needs_refresh`.
pub async fn resolve_token(
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

/// Resolve a public profile without requiring a browser login. Keep the OAuth profile when
/// available for compatibility, but a missing or unusable OAuth grant does not invalidate
/// the service token: the bot can look up the account's stored Discord id instead.
pub async fn user_profile(
    state: &AppState,
    discord_id: i64,
) -> Result<serde_json::Map<String, serde_json::Value>, ApiError> {
    if let Some(authorization) = vc_core::user::find_discord_auth(state.pool(), discord_id).await?
        && let Ok(token) = resolve_token(state, discord_id, &authorization).await
        && let Ok(profile) = state.discord().get_user_info(&token).await
    {
        return Ok(profile);
    }
    state
        .discord()
        .get_user(discord_id)
        .await?
        .ok_or(ApiError::NotFound)
}
