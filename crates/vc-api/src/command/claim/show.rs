use serde_json::{Value, json};
use vc_core::balance::Balance;
use vc_core::claim::{ClaimCurrency, ClaimView};

use super::{format_date_time, render_error, sub_option};
use crate::command::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, CommandError, as_int, get_user, mention,
};
use crate::state::AppState;

/// `Command.handle/4` for `claim show`: one claim, with the buttons its two
/// parties can press.
pub async fn handle(
    state: &AppState,
    sub_options: Option<&Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let id = as_int(sub_option(sub_options, "id")?)
        .ok_or_else(|| CommandError::missing("claim id is not a number"))?;

    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    let Some(claim) = vc_core::claim::view(state.pool(), account, id).await? else {
        return Ok(render_error("そのidの請求は見つかりませんでした。"));
    };

    // `Money.get_claim_by_id/2` answers whoever asks; the handler is what
    // requires the caller to be one of the claim's two parties.
    if claim.claimant.discord_id != Some(me) && claim.payer.discord_id != Some(me) {
        return Ok(render_error("そのidの請求は見つかりませんでした。"));
    }

    // Only a pending claim's payer is shown their balance: the quotation is what
    // tells them whether they can cover it.
    let assets = if claim.status.as_deref() == Some("pending") && claim.payer.discord_id == Some(me)
    {
        Some(vc_core::balance::for_discord_user(state.pool(), me).await?)
    } else {
        None
    };

    Ok(render(&claim, me, assets.as_deref()))
}

/// `Interactions.Claim.Show.render/1`.
fn render(claim: &ClaimView, me: i64, assets: Option<&[Balance]>) -> Value {
    let unit = claim.currency.unit.clone().unwrap_or_default();
    let amount = claim.amount.unwrap_or_default();
    let claimant = claim.claimant.discord_id;
    let payer = claim.payer.discord_id;

    // `currencies.unit` is unique, so finding the balance by the claim's unit is
    // the same as `depends_assets/1` finding it by currency id.
    let current = assets.map(|assets| {
        assets
            .iter()
            .find(|balance| balance.unit == unit)
            .map(|balance| balance.amount)
            .unwrap_or(0)
    });

    // The embed was a title and a field, and a Text Display has no title: it is a bold line above
    // what it introduced. The second embed was the quotation, the same way.
    let heading = format!(
        "**請求**\n**{}{}**",
        render_rs_icon(me, claimant, payer),
        claim.id
    );
    let field = format!(
        "状態　: {}\n請求額: **{amount}** `{unit}`\n請求元: {}\n請求先: {}\n請求日: {}",
        render_status(claim.status.as_deref()),
        mention(claimant.unwrap_or_default()),
        mention(payer.unwrap_or_default()),
        format_date_time(claim.inserted_at),
    );

    let mut children = vec![crate::components::text(format!("{heading}\n{field}"))];

    let mut rows = Vec::new();

    if let Some(current) = current {
        children.push(crate::components::text(format!(
            "**残高**\n{}",
            render_quotation(&claim.currency, current, amount)
        )));
        rows.push(action_row(claim, me, Some(current)));
    } else if claim.status.as_deref() == Some("pending") {
        rows.push(action_row(claim, me, None));
    }

    // The text, then the buttons: a person reads the claim before acting on it.
    children.extend(rows);

    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            // Both embeds carried this accent.
            Some(COLOR_BRAND as u32),
            children,
        )]),
    })
}

/// `Show.selection_execute_row/2`: the three buttons, disabled when the caller
/// could not press them. Without a known balance the approval stays disabled,
/// which is what comparing an amount against `nil` does in Elixir.
fn action_row(claim: &ClaimView, me: i64, current: Option<i64>) -> Value {
    let amount = claim.amount.unwrap_or_default();

    let approve =
        claim.payer.discord_id != Some(me) || current.is_none_or(|current| amount > current);
    let deny = claim.payer.discord_id != Some(me);
    let cancel = claim.claimant.discord_id != Some(me);

    crate::components::action_row(vec![
        crate::components::icon_button(
            &action_custom_id(1, ButtonAction::Approve, claim.id),
            "✅",
            crate::components::ButtonStyle::Success,
            Some(approve),
        ),
        crate::components::icon_button(
            &action_custom_id(2, ButtonAction::Deny, claim.id),
            "❌",
            // Grey rather than red, for the list's reason: the ❌ is a red cross, and a red
            // cross on a red button is the one button on this row nobody can read.
            crate::components::ButtonStyle::Secondary,
            Some(deny),
        ),
        crate::components::icon_button(
            &action_custom_id(3, ButtonAction::Cancel, claim.id),
            "🗑️",
            crate::components::ButtonStyle::Primary,
            Some(cancel),
        ),
    ])
}

use crate::custom_id::ui::button::Action as ButtonAction;

/// `Show.action_custom_id/3`: the button's action id followed by the claim's id
/// as eight big-endian bytes.
fn action_custom_id(k: u8, action: ButtonAction, claim_id: i64) -> String {
    let mut payload = crate::custom_id::ui::button::claim_action_single(action).to_vec();
    payload.extend_from_slice(&claim_id.to_be_bytes());

    crate::custom_id::encode(k, &payload)
}

/// `Show.render_rs_icon/3`: which side of the claim the caller is on.
fn render_rs_icon(me: i64, claimant: Option<i64>, payer: Option<i64>) -> &'static str {
    if claimant == Some(me) && payer == Some(me) {
        "📤📥"
    } else if claimant == Some(me) {
        "📤"
    } else if payer == Some(me) {
        "📥"
    } else {
        // Unreachable: the caller is a party, or the claim was not shown.
        ""
    }
}

/// `Show.render_status/1`.
fn render_status(status: Option<&str>) -> &'static str {
    match status {
        Some("approved") => "✅支払い済み",
        Some("denied") => "❌拒否",
        Some("canceled") => "🗑️キャンセル",
        Some("pending") => "⌛未決定",
        _ => "",
    }
}

/// `Show.render_quotation/1`: what the payer holds, against what is asked, with
/// a warning when it does not cover it.
fn render_quotation(currency: &ClaimCurrency, current: i64, quoted: i64) -> String {
    let unit = currency.unit.clone().unwrap_or_default();
    let name = currency.name.clone().unwrap_or_default();
    let warning = if current < quoted { "⚠" } else { "" };

    format!(
        "**{name}**: `{current}{unit}` - `{quoted}{unit}` => `{}{unit}`{warning}",
        current - quoted
    )
}
