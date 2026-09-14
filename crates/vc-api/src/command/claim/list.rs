use serde_json::{Value, json};
use vc_core::balance::Balance;
use vc_core::claim::{ClaimPage, ClaimView, PageTarget, SrFilter};

use super::format_date_time;
use crate::claim_list::{ListOptions, Page, Position, encode_claim_ids};
use crate::command::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, CommandError, UPDATE_MESSAGE, as_int, get_user,
    mention,
};
use crate::custom_id::ui::button::{Action, ListScope, claim_action, claim_list};
use crate::custom_id::ui::select_menu::claim_select;
use crate::state::AppState;

/// `Listing.@max_column_count`: how many claims a page holds, and where the
/// payloads of the rows below it start counting from.
const MAX_COLUMN_COUNT: u8 = 5;
const LIMIT: i64 = 5;

/// `List.Component.page/2`: re-render the list for the operator, which is the
/// response a button press gets rather than a fresh command.
pub async fn page(state: &AppState, me: i64, options: ListOptions) -> Result<Value, CommandError> {
    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;
    let statuses = statuses_of(&options);
    let related_user = options.related_user.map(|id| id as i64);

    let page = match options.page {
        Page::Number(number) => i64::from(number),
        // `:last` is a page number the caller does not know yet.
        Page::Last => {
            vc_core::claim::last_page_number(
                state.pool(),
                account,
                &statuses,
                sr_filter(options.position),
                related_user,
                LIMIT,
            )
            .await?
        }
    };

    let page = vc_core::claim::list_page(
        state.pool(),
        account,
        &statuses,
        sr_filter(options.position),
        related_user,
        page,
        LIMIT,
    )
    .await?;

    Ok(render(
        options.position,
        &page,
        me,
        &options,
        UPDATE_MESSAGE,
    ))
}

/// `List.extract_statuses/1` for options that already carry the bits.
fn statuses_of(options: &ListOptions) -> Vec<String> {
    let mut statuses: Vec<String> = [
        ("approved", options.approved),
        ("canceled", options.canceled),
        ("denied", options.denied),
        ("pending", options.pending),
    ]
    .into_iter()
    .filter(|(_, set)| *set)
    .map(|(name, _)| name.to_string())
    .collect();

    if statuses.is_empty() {
        statuses.push("pending".to_string());
    }

    statuses
}

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

    Ok(render(
        position,
        &page,
        me,
        &options,
        CHANNEL_MESSAGE_WITH_SOURCE,
    ))
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
fn render(
    position: Position,
    page: &ClaimPage,
    me: i64,
    options: &ListOptions,
    kind: i64,
) -> Value {
    let pending: Vec<&ClaimView> = page
        .claims
        .iter()
        .filter(|claim| claim.status.as_deref() == Some("pending"))
        .collect();

    // The page's title was an embed's title: a bold line, with the list under it.
    let mut children = vec![crate::components::text(format!("**{}**", title(position)))];

    if page.claims.is_empty() {
        children.push(crate::components::text("表示する内容がありません。"));
    } else {
        children.extend(fields(position, &page.claims, me, &[]));
    }

    // The rows first: pagination and the menu change what is on the page rather than say
    // anything about it.
    let mut rows = vec![pagination_row(position, page, options)];

    // The select menu lists what can still be acted on, so it is absent from an
    // empty page.
    if !pending.is_empty() {
        rows.push(select_row(MAX_COLUMN_COUNT, &pending, me, options, &[]));
    }

    // The action row only appears once something has been selected, which a
    // command cannot do.

    children.extend(rows);

    json!({
        "type": kind,
        "data": crate::components::ephemeral(vec![crate::components::container(
            // What both embeds carried.
            Some(COLOR_BRAND as u32),
            children,
        )]),
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
fn fields(position: Position, claims: &[ClaimView], me: i64, selected: &[i64]) -> Vec<Value> {
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

            let name = format!(
                "{}{}{}",
                render_selection(claim.status.as_deref(), selected.contains(&claim.id)),
                render_rs_icon(me, claim.claimant.discord_id, claim.payer.discord_id),
                claim.id
            );

            // The field's name and its value as one Text Display: a component has no `name`.
            crate::components::text(format!("**{name}**\n{}", lines.join("\n")))
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

        crate::components::icon_button(
            &custom_id,
            emoji,
            crate::components::ButtonStyle::Secondary,
            // A page that is not there is a button that says so.
            Some(target.is_none()),
        )
    };

    crate::components::action_row(vec![
        button(0, "⏪", page.first.map(PageTarget::Number)),
        button(1, "⏮️", page.prev.map(PageTarget::Number)),
        button(2, "⏭️", page.next),
        button(3, "⏩", page.last),
        // The reload button is always available, so it carries no `disabled`
        // at all.
        crate::components::icon_button(
            &page_custom_id(4, position, PageTarget::Number(page.page), options),
            "🔄",
            crate::components::ButtonStyle::Secondary,
            None,
        ),
    ])
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
fn select_row(
    k: u8,
    pending: &[&ClaimView],
    me: i64,
    options: &ListOptions,
    selected: &[i64],
) -> Value {
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
                "default": selected.contains(&claim.id),
            })
        })
        .collect();

    crate::components::action_row(vec![crate::components::select_many(
        &crate::custom_id::encode(k, &payload),
        // The options are the claims themselves, so there is no sentence to put above them.
        None,
        choices,
        0,
        u8::try_from(pending.len()).unwrap_or(u8::MAX),
    )])
}

/// One currency's share of a selection: what the operator holds, against what is
/// being asked of them.
struct Quotation {
    name: String,
    unit: String,
    current: i64,
    quoted: i64,
}

fn quotations(selected: &[&ClaimView], me: i64, balances: &[Balance]) -> Vec<Quotation> {
    let mut quotations: Vec<Quotation> = Vec::new();

    for claim in selected {
        let unit = claim.currency.unit.clone().unwrap_or_default();

        let index = match quotations
            .iter()
            .position(|quotation| quotation.unit == unit)
        {
            Some(index) => index,
            None => {
                let current = balances
                    .iter()
                    .find(|balance| balance.unit == unit)
                    .map(|balance| balance.amount)
                    .unwrap_or(0);

                quotations.push(Quotation {
                    name: claim.currency.name.clone().unwrap_or_default(),
                    unit,
                    current,
                    quoted: 0,
                });

                quotations.len() - 1
            }
        };

        // Only what this operator would pay counts against them.
        if claim.payer.discord_id == Some(me) {
            quotations[index].quoted += claim.amount.unwrap_or_default();
        }
    }

    quotations
}

/// `Listing.render_quotation/1`: a lone balance when nothing is asked of them,
/// and the arithmetic with a warning when more is asked than they hold.
fn quotation_text(quotation: &Quotation) -> String {
    let Quotation {
        name,
        unit,
        current,
        quoted,
    } = quotation;

    if *quoted == 0 {
        return format!("**{name}**: `{current}{unit}`");
    }

    let warning = if current < quoted { "⚠" } else { "" };

    format!(
        "**{name}**: `{current}{unit}` - `{quoted}{unit}` => `{}{unit}`{warning}",
        current - quoted
    )
}

/// `Listing.render/2` for `:select`: the page with the selection marked, what
/// the selection would spend, and the buttons that act on it.
pub fn selection(
    position: Position,
    claims: &[ClaimView],
    me: i64,
    options: &ListOptions,
    selected: &[i64],
    balances: &[Balance],
) -> Value {
    let pending: Vec<&ClaimView> = claims
        .iter()
        .filter(|claim| claim.status.as_deref() == Some("pending"))
        .collect();
    let selected_claims: Vec<&ClaimView> = pending
        .iter()
        .copied()
        .filter(|claim| selected.contains(&claim.id))
        .collect();

    let quotations = quotations(&selected_claims, me, balances);

    let mut children = vec![crate::components::text(format!("**{}**", title(position)))];

    if claims.is_empty() {
        children.push(crate::components::text("表示する内容がありません。"));
    } else {
        children.extend(fields(position, claims, me, selected));
    }

    if !quotations.is_empty() {
        children.push(crate::components::text(format!(
            "**残高**\n{}",
            quotations
                .iter()
                .map(quotation_text)
                .collect::<Vec<_>>()
                .join("\n"),
        )));
    }

    let mut rows = Vec::new();

    if !pending.is_empty() {
        rows.push(select_row(0, &pending, me, options, selected));
    }

    if let Some(row) = action_row(&selected_claims, &quotations, me, options) {
        rows.push(row);
    }

    children.extend(rows);

    json!({
        "type": UPDATE_MESSAGE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            Some(COLOR_BRAND as u32),
            children,
        )]),
    })
}

/// `Listing.selection_execute_row/5`: what the selection allows, which is
/// nothing to act on until something is selected.
fn action_row(
    selected: &[&ClaimView],
    quotations: &[Quotation],
    me: i64,
    options: &ListOptions,
) -> Option<Value> {
    if selected.is_empty() {
        return None;
    }

    let cancelable = selected
        .iter()
        .all(|claim| claim.claimant.discord_id == Some(me));
    let deniable = selected
        .iter()
        .all(|claim| claim.payer.discord_id == Some(me));
    let approvable = deniable
        && quotations
            .iter()
            .all(|quotation| quotation.current >= quotation.quoted);

    let ids: Vec<i64> = selected.iter().map(|claim| claim.id).collect();

    let button = |k: u8,
                  emoji: &str,
                  style: crate::components::ButtonStyle,
                  action: Action,
                  disabled: Option<bool>| {
        let mut payload = claim_action(action).to_vec();
        payload.extend_from_slice(&options.encode());
        payload.extend_from_slice(&encode_claim_ids(&ids));

        crate::components::icon_button(
            &crate::custom_id::encode(k, &payload),
            emoji,
            style,
            // Only the actions that can be refused carry the flag; going back is always possible.
            disabled,
        )
    };

    Some(crate::components::action_row(vec![
        button(
            5,
            "⬅️",
            crate::components::ButtonStyle::Secondary,
            Action::Back,
            None,
        ),
        button(
            6,
            "✅",
            crate::components::ButtonStyle::Success,
            Action::Approve,
            Some(!approvable),
        ),
        button(
            7,
            "❌",
            crate::components::ButtonStyle::Danger,
            Action::Deny,
            Some(!deniable),
        ),
        button(
            8,
            "🗑️",
            crate::components::ButtonStyle::Primary,
            Action::Cancel,
            Some(!cancelable),
        ),
    ]))
}
