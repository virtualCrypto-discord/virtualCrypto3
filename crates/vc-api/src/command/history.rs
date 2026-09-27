//! `/history`: the ledgers behind `/issue` and `/pay`, read back.
//!
//! Two subcommands, one screen each, and the arrows between pages — the shape `/bal` and the
//! mute list have. What is being read is `vc_core::history`: the pool's issuances, which
//! `/issue` writes, and the wallet's own movements, which `/pay`, a claim's approval, and a
//! contract's lock and return all write. A charge appears as an incoming payment for its
//! receiver, while the payer's wallet already recorded the lock.
//!
//! The payments screen is the caller's own ledger, and an issuance to them is money arriving in
//! their wallet like any other, so it is on that screen too: both ledgers, merged, newest first.
//! The issuance screen is the guild's one ledger, and is not merged with anything.
//!
//! An addition rather than a port. The Elixir writes both ledgers and never reads either: no
//! command of its names a history, and its `/api/v1|2/…/transactions` are writers.
//! `docs/known-gaps.md` records the addition and `docs/commands.rs` is the prose a person reads.
//!
//! What may be seen follows what may be done: the payments screen is the caller's own and needs
//! nothing, and the issuance screen is the administrator's, like `/issue` beside it — the same
//! bit, read the same way, in the same place.

use serde_json::{Map, Value, json};

use super::claim::format_date_time;
use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, CommandError, UPDATE_MESSAGE, as_int,
    as_permissions, get_user, is_administrator, mention, value_text,
};
use crate::components::{ButtonStyle, action_row, container, ephemeral, icon_button, text};
use crate::custom_id::ui::history::{Listing, Screen, page_custom_id};
use crate::error::ApiError;
use crate::state::AppState;
use vc_core::history::{Issuance, Movement, PER_PAGE, Payment};

/// Where a screen starts, and where an arrow counts from.
const FIRST_PAGE: i64 = 1;

/// `Command.handle/4` for `history`.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let subcommand = options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("history has no subcommand"))?;

    let chosen = options.get("sub_options");
    let listed = chosen
        .and_then(|options| options.get("user"))
        .and_then(as_int);

    match subcommand {
        "pay" => {
            let listing = Listing {
                screen: Screen::Paid,
                page: FIRST_PAGE,
                unit: chosen
                    .and_then(|options| options.get("unit"))
                    .map(value_text),
                discord_id: listed,
            };

            paid(state, &listing, me, CHANNEL_MESSAGE_WITH_SOURCE).await
        }
        "issue" => {
            let listing = Listing {
                screen: Screen::Issued,
                page: FIRST_PAGE,
                unit: None,
                discord_id: listed,
            };

            issued(state, &listing, payload, CHANNEL_MESSAGE_WITH_SOURCE).await
        }
        // Everything this service registers is written down, so a subcommand that is not is one
        // it does not have: a client with a stale command list, answered the way any unknown
        // command is.
        _ => Err(CommandError::Unknown),
    }
}

/// A button on a history screen: the same screen, one page along.
///
/// The screen and its filters travel in the `custom_id`, because Discord sends nothing else
/// back — which is why an arrow cannot lose the filter it was drawn under.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let listing = crate::custom_id::ui::history::parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    match listing.screen {
        Screen::Paid => paid(state, &listing, me, UPDATE_MESSAGE).await,
        Screen::Issued => issued(state, &listing, payload, UPDATE_MESSAGE).await,
    }
}

/// `/history pay`: what the caller sent and received, newest first.
async fn paid(
    state: &AppState,
    listing: &Listing,
    me: i64,
    kind: i64,
) -> Result<Value, CommandError> {
    // The ledger is keyed by account, which is what the API's side of this holds, so a Discord
    // id is resolved first. Somebody who has never used this service has no ledger to show.
    let Some(account) = vc_core::user::find_by_discord_id(state.pool(), me)
        .await?
        .map(|user| user.id)
    else {
        return Ok(answer(vec![text("送金の履歴はありません。")], kind));
    };

    let page = vc_core::history::payments(
        state.pool(),
        account,
        listing.unit.as_deref(),
        listing.discord_id,
        listing.page,
        PER_PAGE,
    )
    .await?;

    let mut children = Vec::new();

    if page.total == 0 {
        children.push(text("送金の履歴はありません。"));
    } else {
        // The count is everything the person sent, received and was issued, not what this page
        // shows: that is the number they are looking for, as it is in every list here.
        children.push(text(format!("**送金の履歴** ({}件)", page.total)));

        if page.rows.is_empty() {
            // A page an arrow led to that the list has since shrunk past: the arrows are the way
            // back, and saying where the rows are beats saying there are none.
            children.push(text(
                "このページには何もありません。前のページに戻ってください。",
            ));
        }

        for movement in &page.rows {
            children.push(text(movement_line(movement, me)));
        }

        if page.next.is_some() || page.page > 1 {
            children.push(arrows(listing, &page));
        }
    }

    Ok(answer(children, kind))
}

/// `/history issue`: what the guild's pool has issued, newest first.
///
/// The administrator's, like `/issue` itself: the pool is the guild's, so who it paid is the
/// business of whoever may spend it.
async fn issued(
    state: &AppState,
    listing: &Listing,
    payload: &Value,
    kind: i64,
) -> Result<Value, CommandError> {
    // Every `Command.handle/4` clause for `give` takes a guild, so a direct message has no
    // handler at all; the same is true here.
    let Some(guild_id) = payload.get("guild_id").and_then(as_int) else {
        return Ok(refused("エラー: DMでは実行できません。"));
    };

    let permissions = payload
        .get("member")
        .and_then(|member| member.get("permissions"))
        .and_then(as_permissions)
        .ok_or_else(|| CommandError::missing("history has no permissions"))?;

    if !is_administrator(permissions) {
        return Ok(refused("エラー: 実行には管理者権限が必要です。"));
    }

    // The ledger is the currency's, which is what the API's path names; the guild is what a
    // screen has, so the currency is looked up. A guild whose pool has never paid anybody — and
    // one that has no currency at all — reads as the empty ledger rather than as an error.
    let Some(currency) = vc_core::history::guild_currency(state.pool(), guild_id).await? else {
        return Ok(answer(
            vec![text(
                "発行の履歴はありません。このサーバーでは、まだ発行枠から発行されていません。",
            )],
            kind,
        ));
    };

    let page = vc_core::history::issuances(
        state.pool(),
        currency,
        listing.discord_id,
        listing.page,
        PER_PAGE,
    )
    .await?;

    let mut children = Vec::new();

    if page.total == 0 {
        children.push(text(
            "発行の履歴はありません。このサーバーでは、まだ発行枠から発行されていません。",
        ));
    } else {
        children.push(text(format!("**発行の履歴** ({}件)", page.total)));

        if page.rows.is_empty() {
            children.push(text(
                "このページには何もありません。前のページに戻ってください。",
            ));
        }

        for issuance in &page.rows {
            children.push(text(issued_line(issuance)));
        }

        if page.next.is_some() || page.page > 1 {
            children.push(arrows(listing, &page));
        }
    }

    Ok(answer(children, kind))
}

/// One row of the caller's own ledger, whichever ledger wrote it.
///
/// The two are one screen because they are one thing to the person reading it: money that arrived
/// and money that left. Which table the row is in is what the line says when it is a 発行 rather
/// than a 受取 — the other end is the pool, not a person.
fn movement_line(movement: &Movement, me: i64) -> String {
    match movement {
        Movement::Payment(payment) => paid_line(payment, me),
        Movement::Issuance(issuance) => issued_to_me(issuance),
    }
}

/// Money the pool issued to the reader: it arrived in the wallet, so it reads as money coming in,
/// and what it came from is the issuance pool rather than a person to mention.
fn issued_to_me(issuance: &Issuance) -> String {
    format!(
        "発行: **{}** `{}` ← 発行枠 ・ {}",
        issuance.amount,
        issuance.unit,
        format_date_time(issuance.time)
    )
}

/// One row of the payments ledger: which way the money went, how much, and when.
///
/// 「送金」 and 「受取」 rather than an arrow each, because that is the question a person opens a
/// ledger with — and the amount is what they are looking for on the line.
///
/// A contract's money is the same ledger, so it needs the same line: a lock is money leaving for
/// a contract and a return is money coming back from one, and both say so in words. What is on
/// the other side is the contract rather than a person — the escrow is nobody's account — so
/// identifying the contract's application tells the reader where the money was delegated.
fn paid_line(payment: &Payment, me: i64) -> String {
    let amount = format!("**{}** `{}`", payment.amount, payment.unit);
    let time = format_date_time(payment.time);

    match payment.event {
        Some("lock") => format!(
            "契約にロック: {amount} → {} ・ {time}",
            contract_identity(payment)
        ),
        Some("return") => format!(
            "契約から返却: {amount} ← {} ・ {time}",
            contract_identity(payment)
        ),
        Some("charge") => format!(
            "契約から受取: {amount} ← {} ・ {time}",
            contract_identity(payment)
        ),
        _ => match (
            payment.sender_discord_id == Some(me),
            payment.receiver_discord_id == Some(me),
        ) {
            (true, false) => format!(
                "送金: {amount} → {} ・ {time}",
                counterparty(payment.receiver_discord_id, payment)
            ),
            (false, true) => format!(
                "受取: {amount} ← {} ・ {time}",
                counterparty(payment.sender_discord_id, payment)
            ),
            // To themselves, which an account may do: the claim list's own fixture holds one.
            _ => format!("{amount} ・ {time}"),
        },
    }
}

/// Use the same identity as the approval screen. A later name edit must not
/// turn a past movement into somebody else's mention or another ledger line.
fn contract_identity(payment: &Payment) -> String {
    match &payment.contract_client_id {
        Some(client_id) => super::application_identity(
            payment.contract_bot_discord_id,
            client_id,
            payment.contract_client_name.as_deref(),
        ),
        None => "契約".to_owned(),
    }
}

/// One row of the issuance ledger: the pool paid somebody, and this is who.
fn issued_line(issuance: &Issuance) -> String {
    let to = match issuance.receiver_discord_id {
        Some(discord_id) => mention(discord_id),
        None => "アプリケーション".to_owned(),
    };

    format!(
        "発行: **{}** `{}` → {to} ・ {}",
        issuance.amount,
        issuance.unit,
        format_date_time(issuance.time)
    )
}

/// Who the other side of a payment is: a mention, or — when the account has no Discord id,
/// which is what an application's account is — the contract that took the money, or the word
/// for what it is.
fn counterparty(discord_id: Option<i64>, payment: &Payment) -> String {
    match discord_id {
        Some(discord_id) => mention(discord_id),
        None => match &payment.contract_client_id {
            Some(_) => contract_identity(payment),
            None => "アプリケーション".to_owned(),
        },
    }
}

/// Where the arrows move to, each disabled where there is nowhere to go — the balance list's
/// row, over a page of the same shape.
fn arrows<T>(listing: &Listing, page: &vc_core::history::Page<T>) -> Value {
    let arrow = |at: u8, emoji: &str, target: Option<i64>| -> Value {
        let id = match target {
            // A page that is not there is a button that says so, and one Discord will not send:
            // the id is a placeholder rather than this space's.
            None => format!("disabled-{at}"),
            Some(number) => page_custom_id(&Listing {
                page: number,
                ..listing.clone()
            }),
        };

        icon_button(&id, emoji, ButtonStyle::Secondary, Some(target.is_none()))
    };

    action_row(vec![
        arrow(0, "⏪", page.first),
        arrow(1, "⏮️", page.prev),
        arrow(2, "⏭️", page.next),
        arrow(3, "⏩", page.last),
    ])
}

/// A screen as a response: ephemeral, and the brand's colour, as every other list's answer is.
fn answer(children: Vec<Value>, kind: i64) -> Value {
    json!({
        "type": kind,
        "data": ephemeral(vec![container(Some(COLOR_BRAND as u32), children)]),
    })
}

/// Refusals are private and carry the shared error accent.
fn refused(sentence: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": ephemeral(vec![container(Some(COLOR_ERROR as u32), vec![text(sentence)])]),
    })
}
