use serde_json::{Value, json};
use vc_core::claim::{ClaimPage, ClaimView, PageTarget, SrFilter};

use super::format_date_time;
use crate::claim_list::{ListOptions, Page, Position, encode_claim_ids};
use crate::command::{
    ACTION_ROW, BUTTON, BUTTON_STYLE_SECONDARY, CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND,
    CommandError, EPHEMERAL, SELECT_MENU, as_int, get_user, mention,
};
use crate::custom_id::ui::button::{ListScope, claim_list};
use crate::custom_id::ui::select_menu::claim_select;
use crate::state::AppState;

/// `Listing.@max_column_count`: how many claims a page holds, and where the
/// payloads of the rows below it start counting from.
const MAX_COLUMN_COUNT: u8 = 5;
const LIMIT: i64 = 5;

/// `Command.handle/4` for `claim list`, `claim received` and `claim sent`.
pub async fn handle(
    state: &AppState,
    sub_options: Option<&Value>,
    payload: &Value,
    position: Position,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    let statuses = statuses(sub_options);
    let related_user = sub_options
        .and_then(|options| options.get("related_user"))
        .and_then(as_int);

    // The command opens on the first page; the buttons move from there.
    let options = ListOptions {
        pending: statuses.iter().any(|status| status == "pending"),
        approved: statuses.iter().any(|status| status == "approved"),
        denied: statuses.iter().any(|status| status == "denied"),
        canceled: statuses.iter().any(|status| status == "canceled"),
        position,
        page: Page::Number(1),
        related_user: related_user.map(|id| id as u64),
    };

    let page = vc_core::claim::list_page(
        state.pool(),
        account,
        &statuses,
        sr_filter(position),
        related_user,
        1,
        LIMIT,
    )
    .await?;

    Ok(render(position, &page, me, &options))
}

/// `List.extract_statuses/1`: the flags that are set, in the order a small map
/// enumerates them, or `pending` when none are.
fn statuses(sub_options: Option<&Value>) -> Vec<String> {
    let mut statuses: Vec<String> = ["approved", "canceled", "denied", "pending"]
        .into_iter()
        .filter(|name| {
            sub_options
                .and_then(|options| options.get(name))
                .and_then(Value::as_bool)
                == Some(true)
        })
        .map(str::to_string)
        .collect();

    if statuses.is_empty() {
        statuses.push("pending".to_string());
    }

    statuses
}

fn sr_filter(position: Position) -> SrFilter {
    match position {
        Position::All => SrFilter::All,
        Position::Received => SrFilter::Received,
        Position::Claimed => SrFilter::Claimed,
    }
}

fn list_scope(position: Position) -> ListScope {
    match position {
        Position::All => ListScope::All,
        Position::Received => ListScope::Received,
        Position::Claimed => ListScope::Claimed,
    }
}

/// `Listing.render/2` for a command or a list button.
fn render(position: Position, page: &ClaimPage, me: i64, options: &ListOptions) -> Value {
    let pending: Vec<&ClaimView> = page
        .claims
        .iter()
        .filter(|claim| claim.status.as_deref() == Some("pending"))
        .collect();

    let embed = json!({
        "title": title(position),
        "color": COLOR_BRAND,
        "fields": fields(position, &page.claims, me),
        "description": if page.claims.is_empty() {
            json!("表示する内容がありません。")
        } else {
            Value::Null
        },
    });

    let mut components = vec![pagination_row(position, page, options)];

    // The select menu lists what can still be acted on, so it is absent from an
    // empty page.
    if !pending.is_empty() {
        components.push(select_row(&pending, me, options));
    }

    // The action row only appears once something has been selected, which a
    // command cannot do.

    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "embeds": [embed],
            "components": components,
        },
    })
}

/// `Listing.render_title/1`.
fn title(position: Position) -> &'static str {
    match position {
        Position::All => "請求一覧(all)",
        Position::Received => "請求一覧(received)",
        Position::Claimed => "請求一覧(sent)",
    }
}

/// `Listing.render_claim/3`.
fn fields(position: Position, claims: &[ClaimView], me: i64) -> Vec<Value> {
    claims
        .iter()
        .map(|claim| {
            let unit = claim.currency.unit.clone().unwrap_or_default();
            let mut lines = vec![
                format!("状態　: {}", render_status(claim.status.as_deref())),
                format!("請求額: **{}** `{unit}`", claim.amount.unwrap_or_default()),
            ];
            lines.extend(users(position, claim));
            lines.push(format!("請求日: {}", format_date_time(claim.inserted_at)));

            json!({
                "name": format!(
                    "{}{}{}",
                    render_selection(claim.status.as_deref(), false),
                    render_rs_icon(me, claim.claimant.discord_id, claim.payer.discord_id),
                    claim.id
                ),
                "value": lines.join("\n"),
            })
        })
        .collect()
}

/// `Listing.render_selection/2`: a pending claim is the only one that can be
/// ticked, so every other status contributes nothing.
fn render_selection(status: Option<&str>, selected: bool) -> &'static str {
    match (status, selected) {
        (Some("pending"), true) => "☑",
        (Some("pending"), false) => "◻️",
        _ => "",
    }
}

fn render_status(status: Option<&str>) -> &'static str {
    match status {
        Some("approved") => "✅支払い済み",
        Some("denied") => "❌拒否",
        Some("canceled") => "🗑️キャンセル",
        Some("pending") => "⌛未決定",
        _ => "",
    }
}

/// `Listing.render_user/3`: which side of the claim the position shows.
fn users(position: Position, claim: &ClaimView) -> Vec<String> {
    let claimant = mention(claim.claimant.discord_id.unwrap_or_default());
    let payer = mention(claim.payer.discord_id.unwrap_or_default());

    match position {
        Position::All => vec![format!("請求元: {claimant}"), format!("請求先: {payer}")],
        Position::Received => vec![format!("請求元: {claimant}")],
        Position::Claimed => vec![format!("請求先: {payer}")],
    }
}

fn render_rs_icon(me: i64, claimant: Option<i64>, payer: Option<i64>) -> &'static str {
    if claimant == Some(me) && payer == Some(me) {
        "📤📥"
    } else if claimant == Some(me) {
        "📤"
    } else if payer == Some(me) {
        "📥"
    } else {
        ""
    }
}

/// `Listing.pagination_row/3`.
fn pagination_row(position: Position, page: &ClaimPage, options: &ListOptions) -> Value {
    let button = |k: u8, emoji: &str, target: Option<PageTarget>| -> Value {
        let custom_id = match target {
            None => format!("disabled-{k}"),
            Some(target) => page_custom_id(k, position, target, options),
        };

        json!({
            "type": BUTTON,
            "style": BUTTON_STYLE_SECONDARY,
            "emoji": { "name": emoji },
            "custom_id": custom_id,
            "disabled": target.is_none(),
        })
    };

    json!({
        "type": ACTION_ROW,
        "components": [
            button(0, "⏪", page.first.map(PageTarget::Number)),
            button(1, "⏮️", page.prev.map(PageTarget::Number)),
            button(2, "⏭️", page.next),
            button(3, "⏩", page.last),
            // The reload button is always available, so it carries no `disabled`
            // at all.
            {
                "type": BUTTON,
                "style": BUTTON_STYLE_SECONDARY,
                "emoji": { "name": "🔄" },
                "custom_id": page_custom_id(4, position, PageTarget::Number(page.page), options),
            },
        ],
    })
}

/// `Listing.custom_id/4` for a page: the list's position and the options with the
/// page the button moves to.
fn page_custom_id(k: u8, position: Position, target: PageTarget, options: &ListOptions) -> String {
    let page = match target {
        PageTarget::Last => Page::Last,
        PageTarget::Number(number) => Page::Number(u32::try_from(number).unwrap_or(u32::MAX)),
    };

    let options = ListOptions { page, ..*options };

    let mut payload = claim_list(list_scope(position)).to_vec();
    payload.extend_from_slice(&options.encode());

    crate::custom_id::encode(k, &payload)
}

/// `Listing.selection_select_row/5`: the claims that can still be acted on, with
/// the options and the ids they are, so the selection comes back with them.
fn select_row(pending: &[&ClaimView], me: i64, options: &ListOptions) -> Value {
    let ids: Vec<i64> = pending.iter().map(|claim| claim.id).collect();

    let mut payload = claim_select().to_vec();
    payload.extend_from_slice(&options.encode());
    payload.extend_from_slice(&encode_claim_ids(&ids));

    let choices: Vec<Value> = pending
        .iter()
        .map(|claim| {
            json!({
                "label": format!(
                    "{}{}",
                    render_rs_icon(me, claim.claimant.discord_id, claim.payer.discord_id),
                    claim.id
                ),
                "value": claim.id.to_string(),
                "description": format!(
                    "{} {}",
                    claim.amount.unwrap_or_default(),
                    claim.currency.unit.clone().unwrap_or_default()
                ),
                "default": false,
            })
        })
        .collect();

    json!({
        "type": ACTION_ROW,
        "components": [{
            "type": SELECT_MENU,
            "custom_id": crate::custom_id::encode(MAX_COLUMN_COUNT, &payload),
            "max_values": pending.len(),
            "min_values": 0,
            "options": choices,
        }],
    })
}
