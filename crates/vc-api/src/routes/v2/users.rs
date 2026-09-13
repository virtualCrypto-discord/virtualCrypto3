use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};
use vc_auth::AuthUser;

use crate::discord::filter_profile;
use crate::discord_auth::resolve_token;
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

/// `GET /api/v2/users/@me/balances`
///
/// The account's holdings, every number as a string, which is what the captured
/// golden shows: `amount` and `pool_amount` are strings and the currency's guild
/// is one too.
pub async fn balances(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let local_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    let account = vc_core::user::find_by_id(state.pool(), local_id)
        .await?
        .ok_or(vc_core::Error::UserNotFound(user.subject))?;

    let discord_id = account
        .discord_id
        .ok_or_else(|| ApiError::Internal(format!("user {local_id} has no discord id")))?;

    let holdings = vc_core::balance::holdings_for_discord_user(state.pool(), discord_id).await?;

    Ok(Json(Value::Array(
        holdings.iter().map(render_holding).collect(),
    )))
}

/// One holding, in the shape the golden shows.
fn render_holding(holding: &vc_core::balance::Holding) -> Value {
    json!({
        "amount": holding.amount.map(|amount| amount.to_string()),
        "currency": {
            "guild": holding.guild.map(|guild| guild.to_string()),
            "name": holding.name,
            "pool_amount": holding.pool_amount.map(|amount| amount.to_string()),
            "unit": holding.unit,
        }
    })
}
