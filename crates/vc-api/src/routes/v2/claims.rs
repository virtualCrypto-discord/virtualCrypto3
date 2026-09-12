use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};
use time::PrimitiveDateTime;
use vc_auth::AuthUser;
use vc_core::claim::ClaimView;

use crate::discord::filter_profile;
use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/v2/users/@me/claims/:id`
///
/// Mirrors `VirtualCryptoWeb.Api.V2.ClaimController.get_by_id/2`: only the payer
/// or the claimant may read a claim, and the metadata is the requester's own.
pub async fn get_by_id(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if !user.scopes.vc_claim {
        return Err(ApiError::PermissionDenied);
    }

    let operator_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    // Ecto would cast the raw path segment into the bigint column and raise on a
    // non-numeric value; PATCH already answers those with 404, so do the same.
    let claim_id: i64 = id.parse().map_err(|_| ApiError::NotFound)?;

    let view = vc_core::claim::view(state.pool(), operator_id, claim_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if view.payer.id != operator_id && view.claimant.id != operator_id {
        return Err(ApiError::Forbidden("not_related_user"));
    }

    Ok(Json(serialize_claim(&state, view).await?))
}

/// `format_claim/2`: amounts and ids as strings, timestamps as UTC with a `Z`,
/// and the claimant/payer decorated with their filtered Discord profile.
pub async fn serialize_claim(state: &AppState, view: ClaimView) -> Result<Value, ApiError> {
    let claimant_discord = discord_user(state, view.claimant.discord_id).await?;
    let payer_discord = discord_user(state, view.payer.discord_id).await?;

    Ok(json!({
        "id": view.id.to_string(),
        "currency": {
            "name": view.currency.name,
            "unit": view.currency.unit,
            "guild": view.currency.guild_id.map(|id| id.to_string()).unwrap_or_default(),
            "pool_amount": view.currency.pool_amount.map(|amount| amount.to_string()).unwrap_or_default(),
        },
        "amount": view.amount.map(|amount| amount.to_string()).unwrap_or_default(),
        "claimant": {
            "id": view.claimant.id.to_string(),
            "discord": claimant_discord,
        },
        "payer": {
            "id": view.payer.id.to_string(),
            "discord": payer_discord,
        },
        "created_at": format_timestamp(view.inserted_at),
        "updated_at": format_timestamp(view.updated_at),
        "status": view.status,
        "metadata": view.metadata,
    }))
}

async fn discord_user(state: &AppState, discord_id: Option<i64>) -> Result<Value, ApiError> {
    let Some(discord_id) = discord_id else {
        return Ok(Value::Null);
    };

    match state.discord().get_user(discord_id).await? {
        Some(payload) => Ok(Value::Object(filter_profile(payload).into_iter().collect())),
        // `Filtering.user/1` is called on the cached `:not_found` atom and raises.
        None => Err(ApiError::Internal(format!(
            "discord user {discord_id} not found"
        ))),
    }
}

/// `DateTime.from_naive!(naive, "Etc/UTC")` serialized by Jason.
fn format_timestamp(value: PrimitiveDateTime) -> String {
    let format =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

    value.format(&format).unwrap_or_default()
}
