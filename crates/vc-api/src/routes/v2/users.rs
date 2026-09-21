use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::routes::limited::Limited;

use crate::discord::filter_profile;
use crate::discord_auth::user_profile;
use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/v2/users/@me`
///
/// Mirrors `VirtualCryptoWeb.Api.V1V2.UserController.me/2`: resolve the local
/// user, take its stored Discord authorization (refreshing it first when it is
/// near expiry) or use the bot lookup when no usable authorization exists, and return
/// `{"id": "<local user id>", "discord": <filtered profile>}`.
///
/// `@me` is whoever holds the token, which is not only a person: an application's token asks
/// about the application's own account, and that account has no Discord behind it. That case is
/// `{"id": …, "discord": null}` rather than an error — there is nothing to read, and a caller
/// asking about itself is not asking for something that is missing.
pub async fn me(State(state): State<AppState>, user: Limited) -> Result<Json<Value>, ApiError> {
    let local_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    let account = vc_core::user::find_by_id(state.pool(), local_id)
        .await?
        .ok_or(vc_core::Error::UserNotFound(user.subject))?;

    let Some(discord_id) = account.discord_id else {
        return Ok(Json(json!({
            "id": account.id.to_string(),
            "discord": Value::Null,
        })));
    };

    let profile = user_profile(&state, discord_id).await?;

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
    user: Limited,
) -> Result<Json<Value>, ApiError> {
    let local_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    let account = vc_core::user::find_by_id(state.pool(), local_id)
        .await?
        .ok_or(vc_core::Error::UserNotFound(user.subject))?;

    let holdings = vc_core::balance::holdings_for_user(state.pool(), account.id).await?;

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
