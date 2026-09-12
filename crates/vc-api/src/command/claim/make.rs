use serde_json::{Value, json};
use vc_core::claim::CreateError;

use super::{render_error, sub_option};
use crate::command::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_OK, CommandError, EPHEMERAL, as_int, get_user, value_text,
};
use crate::state::AppState;

/// `Command.handle/4` for `claim make`: the caller asks someone to pay them.
pub async fn handle(
    state: &AppState,
    sub_options: Option<&Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let claimant =
        get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let payer = value_text(sub_option(sub_options, "user")?);
    let unit = value_text(sub_option(sub_options, "unit")?);
    let amount = as_int(sub_option(sub_options, "amount")?)
        .ok_or_else(|| CommandError::missing("claim amount is not a number"))?;

    let payer_discord_id: i64 = payer
        .parse()
        .map_err(|_| CommandError::missing("claim payer is not an id"))?;

    let claimant_id = vc_core::user::resolve_discord_id(state.pool(), claimant).await?;

    // A command creates a claim without metadata.
    match vc_core::claim::create(
        state.pool(),
        claimant_id,
        payer_discord_id,
        &unit,
        amount,
        None,
    )
    .await
    {
        Ok(claim_id) => Ok(render(claim_id)),
        Err(CreateError::NotFoundCurrency) => Ok(render_error("指定された通貨は存在しません。")),
        Err(CreateError::InvalidAmount) => Ok(render_error(
            "不正な金額です。1以上9223372036854775807以下である必要があります。",
        )),
        Err(CreateError::Database(error)) => Err(CommandError::from(error)),
    }
}

/// `Interactions.Claim.render/1` for `{:ok, "make", claim}`.
fn render(claim_id: i64) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "embeds": [{
                "description": format!(
                    "請求id: {claim_id} で請求を受け付けました。`/claim show id:{claim_id}`でご確認ください。"
                ),
                "color": COLOR_OK,
            }],
        },
    })
}
