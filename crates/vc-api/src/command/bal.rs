//! `Command.handle/4` for `bal`, rendered by `InteractionsJSON.bal/1` through
//! `Interactions.Bal.render/1`.
//!
//! The Elixir answered one `yaml` fence with every currency in a message, and a person can
//! hold as many currencies as they are in guilds — forty rows of them is a message Discord
//! refuses rather than one somebody scrolls. This pages instead, with the four arrows the
//! lists beside it use. What each line says is the Elixir's, word for word, and so is the
//! sentence for holding nothing.

use serde_json::{Value, json};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, CommandError, UPDATE_MESSAGE, get_user, money_text,
};
use crate::components::{ButtonStyle, action_row, container, ephemeral, icon_button, text};
use crate::custom_id::ui::bal::{Page, page_custom_id};
use crate::error::ApiError;
use crate::state::AppState;

/// How many currencies one screen shows, for the lists' reason: ten fits in a message
/// without scrolling it off the screen, and these rows are two lines each.
const MAX_BALANCES: i64 = 10;

/// Where the screen starts, and where an arrow counts from.
const FIRST_PAGE: i64 = 1;

/// `Interactions.Bal.render/1` for the command.
pub async fn handle(state: &AppState, payload: &Value) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    page(state, me, FIRST_PAGE, CHANNEL_MESSAGE_WITH_SOURCE).await
}

/// A page button pressed: the same screen, one page along.
///
/// Discord sends nothing back but the `custom_id`, which is why the page travelled in it.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let (_, number) = crate::custom_id::ui::bal::parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    page(state, me, number, UPDATE_MESSAGE).await
}

/// The screen: ten of what the caller holds, the whole count, and the arrows to the rest.
async fn page(state: &AppState, me: i64, page: i64, kind: i64) -> Result<Value, CommandError> {
    let balances =
        vc_core::balance::page_for_discord_user(state.pool(), me, page, MAX_BALANCES).await?;

    let mut children = Vec::new();

    if balances.total == 0 {
        children.push(text(message!("command.bal.page.001")));
    } else {
        // Count every visible holding, including those on later pages.
        children.push(text(format!(
            message!("command.bal.page.002"),
            balances.total
        )));

        if balances.balances.is_empty() {
            // A page a button led to that the list has since shrunk past: the arrows below are
            // the way back, and saying where the rows are is better than saying there are none.
            children.push(text(message!("command.bal.page.003")));
        }

        for balance in &balances.balances {
            children.push(text(format!(
                "**{}**\n{}",
                balance.name,
                money_text(balance.amount, &balance.unit)
            )));
        }

        if balances.next.is_some() || balances.page > 1 {
            children.push(pagination_row(&balances));
        }
    }

    Ok(json!({
        "type": kind,
        "data": ephemeral(vec![container(Some(COLOR_BRAND as u32), children)]),
    }))
}

/// Where the arrows move to, each disabled where there is nowhere to go.
fn pagination_row(balances: &vc_core::balance::Balances) -> Value {
    let arrow = |at: u8, emoji: &str, target: Option<i64>, page: Page| -> Value {
        let id = match target {
            // A page that is not there is a button that says so, and one Discord will not
            // send: the id is a placeholder rather than this space's.
            None => format!("disabled-{at}"),
            Some(number) => page_custom_id(page, number),
        };

        icon_button(&id, emoji, ButtonStyle::Secondary, Some(target.is_none()))
    };

    action_row(vec![
        arrow(0, "⏪", balances.first, Page::First),
        arrow(1, "⏮️", balances.prev, Page::Previous),
        arrow(2, "⏭️", balances.next, Page::Next),
        arrow(3, "⏩", balances.last, Page::Last),
    ])
}
