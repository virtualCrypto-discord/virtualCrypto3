use serde_json::{Value, json};
use vc_core::balance::Balance;
use vc_core::claim::{ClaimPage, ClaimView, PageTarget, SrFilter};

use super::{format_date_time, user_identity};
use crate::claim_list::{ListOptions, Page, Position, encode_claim_ids};
use crate::command::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, CommandError, UPDATE_MESSAGE, as_int, get_user,
    money_text,
};
use crate::custom_id::ui::button::{Action, ListScope, claim_action, claim_list};
use crate::state::AppState;

/// `Listing.@max_column_count`: how many claims one page holds.
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

    // The caller's own balances, because a row's approval button has to know whether the
    // money is there before it can be offered as pressable.
    let balances = vc_core::balance::for_discord_user(state.pool(), me).await?;

    Ok(render(
        options.position,
        &page,
        me,
        &options,
        &balances,
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
    // The option Discord sends is `user` — the name the registration carries, and
    // the one the screen shows — and what it holds is a Discord id, which the
    // filter is not about: `related_user_id` is a virtualCrypto user, so it is
    // resolved the way the API resolves its own `related_discord_user_id`.
    //
    // This read `related_user` and passed a Discord id where an account id was
    // wanted, so the filter was offered, typed, and ignored. A Discord user with
    // no account here has no claims, and the filter matches nobody rather than
    // showing everything: an unfiltered list is the one answer that would be
    // wrong.
    let related_user = match sub_options
        .and_then(|options| options.get("user"))
        .and_then(as_int)
    {
        Some(discord_id) => {
            let account = vc_core::user::find_by_discord_id(state.pool(), discord_id).await?;

            Some(account.map_or(-1, |account| i64::from(account.id)))
        }
        None => None,
    };

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

    let balances = vc_core::balance::for_discord_user(state.pool(), me).await?;

    Ok(render(
        position,
        &page,
        me,
        &options,
        &balances,
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
    balances: &[Balance],
    kind: i64,
) -> Value {
    // The page's title was an embed's title: a bold line, with the list under it.
    let mut children = vec![crate::components::text(format!("**{}**", title(position)))];

    if page.claims.is_empty() {
        children.push(crate::components::separator());
        children.push(crate::components::text(message!(
            "command.claim.list.render.001"
        )));
    } else {
        children.extend(rows(position, &page.claims, me, options, balances));
    }

    // The rows first: pagination changes what is on the page rather than saying anything
    // about it.
    children.push(crate::components::separator());
    children.push(pagination_row(position, page, options));

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
        Position::All => message!("command.claim.list.title.001"),
        Position::Received => message!("command.claim.list.title.002"),
        Position::Claimed => message!("command.claim.list.title.003"),
    }
}

/// `Listing.render_claim/3`, one claim at a time: the lines it shows, then the buttons its
/// own row offers.
///
/// A pending claim can be answered from its own row, under the same rules its own screen
/// uses. A claim that is already decided shows no buttons: a screen that offers what it will
/// refuse is a screen that lies.
fn rows(
    position: Position,
    claims: &[ClaimView],
    me: i64,
    options: &ListOptions,
    balances: &[Balance],
) -> Vec<Value> {
    let mut children = Vec::new();

    for claim in claims {
        // Keep the claim and its actions together, with the same dividers as history.
        children.push(crate::components::separator());
        children.push(field(position, claim, me));

        if let Some(row) = claim_row(claim, me, options, balances) {
            children.push(row);
        }
    }

    children
}

/// One claim's lines, as a Text Display: a component has no `name` for the field the
/// Elixir's embed carried it as.
fn field(position: Position, claim: &ClaimView, me: i64) -> Value {
    let unit = claim.currency.unit.clone().unwrap_or_default();
    let mut lines = vec![
        format!(
            message!("command.claim.list.field.001"),
            render_status(claim.status.as_deref())
        ),
        format!(
            message!("command.claim.list.field.002"),
            money_text(claim.amount.unwrap_or_default(), &unit)
        ),
    ];
    lines.extend(users(position, claim));
    lines.push(format!(
        message!("command.claim.list.field.003"),
        format_date_time(claim.inserted_at)
    ));

    let name = format!(
        "{}{}",
        render_rs_icon(me, claim.claimant.discord_id, claim.payer.discord_id),
        claim.id
    );

    crate::components::text(format!("**{name}**\n{}", lines.join("\n")))
}

/// The three buttons one claim offers, under the same rules its own screen uses: an approval
/// belongs to the payer and only with the money, a refusal belongs to the payer, and taking
/// the claim back belongs to the claimant.
///
/// The payload is the bulk action's with a single id in it, so a press goes through the same
/// handler a selection does and the list is redrawn the same way.
fn claim_row(
    claim: &ClaimView,
    me: i64,
    options: &ListOptions,
    balances: &[Balance],
) -> Option<Value> {
    if claim.status.as_deref() != Some("pending") {
        return None;
    }

    let unit = claim.currency.unit.clone().unwrap_or_default();
    let current = balances
        .iter()
        .find(|balance| balance.unit == unit)
        .map(|balance| balance.amount)
        .unwrap_or(0);

    let payer = claim.payer.discord_id == Some(me);
    let claimant = claim.claimant.discord_id == Some(me);
    let amount = claim.amount.unwrap_or_default();

    let button = |k: u8,
                  emoji: &str,
                  style: crate::components::ButtonStyle,
                  action: Action,
                  disabled: bool| {
        let mut payload = claim_action(action).to_vec();
        payload.extend_from_slice(&options.encode());
        payload.extend_from_slice(&encode_claim_ids(&[claim.id]));

        crate::components::icon_button(
            &crate::custom_id::encode(k, &payload),
            emoji,
            style,
            Some(disabled),
        )
    };

    Some(crate::components::action_row(vec![
        button(
            5,
            "✅",
            crate::components::ButtonStyle::Success,
            Action::Approve,
            !(payer && current >= amount),
        ),
        // Grey rather than red, as the selection's row is: the ❌ is a red cross, and a red
        // cross on a red button is the one button nobody can read.
        button(
            6,
            "❌",
            crate::components::ButtonStyle::Secondary,
            Action::Deny,
            !payer,
        ),
        button(
            7,
            "🗑️",
            crate::components::ButtonStyle::Primary,
            Action::Cancel,
            !claimant,
        ),
    ]))
}

fn render_status(status: Option<&str>) -> &'static str {
    match status {
        Some("approved") => message!("command.claim.list.render_status.001"),
        Some("denied") => message!("command.claim.list.render_status.002"),
        Some("canceled") => message!("command.claim.list.render_status.003"),
        Some("pending") => message!("command.claim.list.render_status.004"),
        _ => "",
    }
}

/// `Listing.render_user/3`: which side of the claim the position shows.
fn users(position: Position, claim: &ClaimView) -> Vec<String> {
    let claimant = user_identity(&claim.claimant);
    let payer = user_identity(&claim.payer);

    match position {
        Position::All => vec![
            format!(
                message!("command.claim.list.users.001"),
                claimant = claimant
            ),
            format!(message!("command.claim.list.users.002"), payer = payer),
        ],
        Position::Received => vec![format!(
            message!("command.claim.list.users.003"),
            claimant = claimant
        )],
        Position::Claimed => vec![format!(
            message!("command.claim.list.users.004"),
            payer = payer
        )],
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
