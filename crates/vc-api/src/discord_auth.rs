//! The Discord authorization an account holds, and using it.
//!
//! Shared by the v2 user endpoint and by application registration, which need the
//! same thing from it: a token to ask Discord with. It lives here rather than in
//! either of them because it is two constants and a comparison — the fifteen
//! minutes and the seven days — and a second copy is a second place for both to be
//! got wrong, in a flow where getting them wrong means asking Discord with a token
//! that has expired.

use time::Duration;

use vc_core::model::{DiscordAuth, utc_now};

use crate::error::ApiError;
use crate::state::AppState;

/// Reproduces `DiscordAuth.refresh_user/1` followed by the controller's use of
/// the returned struct: refresh inside the final 15 minutes of the seven-day
/// lifetime, store the new authorization, and use the refreshed token.
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
