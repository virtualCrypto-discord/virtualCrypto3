//! `/mute`: what a person has chosen not to see.
//!
//! Two targets and a list. A mute is set by name — a currency's unit, or somebody Discord names
//! for you — and removed with the corresponding button on `/mute list`.
//!
//! A filter rather than a lock, so nothing here answers with a refusal on somebody's behalf:
//! [`vc_core::mute`] is where the lists' own statements ask what to leave out.

use serde_json::{Map, Value, json};
use time::OffsetDateTime;

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, COLOR_OK, CommandError, UPDATE_MESSAGE,
    as_int, error_text, get_user, mention, value_text,
};
use crate::components::{
    ButtonStyle, action_row, button, container, ephemeral, icon_button, section, text,
};
use crate::custom_id::ui::mute::{Page, Pressed, page_custom_id, unmute_custom_id};
use crate::docs::discord::mentions;
use crate::error::ApiError;
use crate::state::AppState;
use vc_core::mute::{MuteError, PER_PAGE, Target};

/// Where a screen starts, and where an arrow counts from.
const FIRST_PAGE: i64 = 1;

/// `Command.handle/4` for `mute`.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let subcommand = subcommand(options, "mute")?;
    let chosen = options.get("sub_options");

    match subcommand {
        "currency" => currency(state, chosen, me).await,
        "user" => user(state, chosen, me).await,
        "list" => list(state, me, FIRST_PAGE, CHANNEL_MESSAGE_WITH_SOURCE).await,
        _ => Err(CommandError::Unknown),
    }
}

/// A button on the mutes screen: one row's button, or one of the arrows.
///
/// The list is drawn from its first page rather than from the page a row was removed on: the row
/// is gone once the press returns, so the page it was on may not be a page at all any more, and
/// the first page is always one.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let Some(account) = account(state, me).await? else {
        return Ok(screen(
            vec![text(message!("command.mute.component.001"))],
            COLOR_BRAND,
        ));
    };

    let pressed = crate::custom_id::ui::mute::parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    match pressed {
        Pressed::Currency(id) => {
            vc_core::mute::unmute_currency_id(state.pool(), account, id)
                .await
                .map_err(database)?;
        }
        Pressed::User(discord_id) => {
            vc_core::mute::unmute_user(state.pool(), account, discord_id)
                .await
                .map_err(database)?;
        }
        Pressed::Paged(_, number) => return page(state, account, number, UPDATE_MESSAGE).await,
    }

    page(state, account, FIRST_PAGE, UPDATE_MESSAGE).await
}

/// `/mute currency unit:<単位>`: one currency, left out of the caller's lists.
async fn currency(
    state: &AppState,
    chosen: Option<&Value>,
    me: i64,
) -> Result<Value, CommandError> {
    let unit = option(chosen, "unit", "mute option unit")?;

    let Some(account) = account(state, me).await? else {
        return Ok(error_screen(message!("command.mute.no_account.001")));
    };

    let muted =
        vc_core::mute::mute_currency(state.pool(), account, &unit, OffsetDateTime::now_utc()).await;

    match muted {
        Ok(true) => Ok(screen(
            vec![text(mentions(
                &format!(message!("command.mute.currency.001"), unit = unit),
                state.command_ids().await,
            ))],
            COLOR_OK,
        )),
        Ok(false) => Ok(screen(
            vec![text(format!(
                message!("command.mute.currency.002"),
                unit = unit
            ))],
            COLOR_BRAND,
        )),
        Err(MuteError::NoSuchCurrency) => Ok(error_screen(&format!(
            message!("command.mute.currency.003"),
            unit = unit
        ))),
        Err(error) => Err(database(error)),
    }
}

/// `/mute user user:<相手>`: one person, left out of the caller's lists.
async fn user(state: &AppState, chosen: Option<&Value>, me: i64) -> Result<Value, CommandError> {
    let Some(target) = chosen
        .and_then(|options| options.get("user"))
        .and_then(as_int)
    else {
        return Err(CommandError::missing("mute option user"));
    };

    let Some(account) = account(state, me).await? else {
        return Ok(error_screen(message!("command.mute.no_account.001")));
    };

    let muted =
        vc_core::mute::mute_user(state.pool(), account, target, OffsetDateTime::now_utc()).await;

    match muted {
        Ok(true) => Ok(screen(
            vec![text(mentions(
                &format!(message!("command.mute.user.001"), mention(target)),
                state.command_ids().await,
            ))],
            COLOR_OK,
        )),
        Ok(false) => Ok(screen(
            vec![text(format!(
                message!("command.mute.user.002"),
                mention(target)
            ))],
            COLOR_BRAND,
        )),
        // Nothing of theirs can be in a list before they have an account here, so there is no
        // mute to make: said as what is missing rather than as a refusal.
        Err(MuteError::NoSuchUser) => Ok(error_screen(&format!(
            message!("command.mute.user.003"),
            mention(target)
        ))),
        Err(MuteError::Yourself) => Ok(error_screen(message!("command.mute.user.004"))),
        Err(error) => Err(database(error)),
    }
}

/// `/mute list`: what the caller is not seeing, a page at a time.
async fn list(state: &AppState, me: i64, number: i64, kind: i64) -> Result<Value, CommandError> {
    let Some(account) = account(state, me).await? else {
        return Ok(no_account());
    };

    page(state, account, number, kind).await
}

/// The screen itself, once there is an account to read it for: a heading, one section per mute
/// with the button that takes it back, and the arrows to the rest.
async fn page(
    state: &AppState,
    account: i32,
    number: i64,
    kind: i64,
) -> Result<Value, CommandError> {
    let mutes = vc_core::mute::page(state.pool(), account, number, PER_PAGE)
        .await
        .map_err(database)?;

    let mut children = Vec::new();

    if mutes.total == 0 {
        children.push(text(mentions(
            message!("command.mute.page.001"),
            state.command_ids().await,
        )));
    } else {
        // The count is everything the person is not seeing, not what this page shows: that is
        // the number they are looking for, as it is in every list here.
        children.push(text(format!(
            message!("command.mute.page.002"),
            mutes.total
        )));

        if mutes.mutes.is_empty() {
            // A page an arrow led to that the list has since shrunk past: the arrows are the way
            // back, and saying where the rows are beats saying there are none.
            children.push(text(message!("command.mute.page.003")));
        }

        for mute in &mutes.mutes {
            children.push(row(mute));
        }

        if mutes.next.is_some() || mutes.page > 1 {
            children.push(arrows(&mutes));
        }
    }

    Ok(json!({
        "type": kind,
        "data": ephemeral(vec![container(Some(COLOR_BRAND as u32), children)]),
    }))
}

/// One row: what is not being seen, and the button that puts it back.
///
/// The button is the section's accessory rather than a row under it, which is what pairs the two
/// up: five 「解除」 buttons below five lines would be five buttons nobody could tell apart.
fn row(mute: &Target) -> Value {
    let line = match mute {
        Target::Currency { name, unit, .. } => format!("**{name}**（`{unit}`）"),
        Target::User { discord_id } => mention(*discord_id),
    };

    section(
        vec![text(line)],
        button(
            &unmute_custom_id(mute),
            message!("command.mute.row.001"),
            ButtonStyle::Secondary,
        ),
    )
}

/// Where the arrows move to, each disabled where there is nowhere to go — the balance list's
/// row, over a list of the same size.
fn arrows(mutes: &vc_core::mute::Mutes) -> Value {
    let arrow = |at: u8, emoji: &str, target: Option<i64>, page: Page| -> Value {
        let id = match target {
            // A page that is not there is a button that says so, and one Discord will not send:
            // the id is a placeholder rather than this space's.
            None => format!("disabled-{at}"),
            Some(number) => page_custom_id(page, number),
        };

        icon_button(&id, emoji, ButtonStyle::Secondary, Some(target.is_none()))
    };

    action_row(vec![
        arrow(0, "⏪", mutes.first, Page::First),
        arrow(1, "⏮️", mutes.prev, Page::Previous),
        arrow(2, "⏭️", mutes.next, Page::Next),
        arrow(3, "⏩", mutes.last, Page::Last),
    ])
}

/// The subcommand an interaction carried. Everything this service registers is written down, so
/// a name that is not one is a client with a stale list rather than a person.
fn subcommand<'a>(options: &'a Map<String, Value>, command: &str) -> Result<&'a str, CommandError> {
    options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing(&format!("{command} has no subcommand")))
}

/// A subcommand's own option, as the text the person typed.
fn option(chosen: Option<&Value>, name: &str, missing: &str) -> Result<String, CommandError> {
    chosen
        .and_then(|options| options.get(name))
        .map(value_text)
        .ok_or_else(|| CommandError::missing(missing))
}

/// The account behind an interaction, or nothing when the caller has none yet — a mute belongs
/// to an account, so an account has to exist before one can be set.
async fn account(state: &AppState, discord_id: i64) -> Result<Option<i32>, CommandError> {
    Ok(vc_core::user::find_by_discord_id(state.pool(), discord_id)
        .await?
        .map(|user| user.id))
}

/// A screen for the caller who has no account here: every answer in this module is about what
/// their own lists show, and somebody with no account has none.
fn no_account() -> Value {
    screen(
        vec![text(message!("command.mute.no_account.001"))],
        COLOR_BRAND,
    )
}

/// A database failure as the interaction layer answers one: the only refusal in
/// [`MuteError`] that is not a sentence is the database's own, and it belongs to `vc_core`.
fn database(error: MuteError) -> CommandError {
    match error {
        MuteError::Database(error) => CommandError::from(vc_core::Error::Database(error)),
        other => CommandError::Internal(ApiError::Internal(format!("{other:?}"))),
    }
}

/// A refusal: the screen below in the error accent, with the heading every error screen has.
fn error_screen(sentence: &str) -> Value {
    screen(vec![text(error_text(sentence))], COLOR_ERROR)
}

/// An ephemeral screen in one colour, which is the shape every answer here has: a mute is one
/// person's own and nobody else in the channel has any business reading it.
fn screen(children: Vec<Value>, color: i64) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": ephemeral(vec![container(Some(color as u32), children)]),
    })
}
