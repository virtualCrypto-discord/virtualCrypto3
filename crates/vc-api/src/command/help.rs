//! `/help`: the list of commands, one command in full, and the menu between them.
//!
//! The screens are [`crate::docs::discord`]'s; this module is the interaction side
//! of them — which screen an option asks for, and which screen a press moves to.
//!
//! Not from the Elixir, whose `help` was one line of links: the two screens and
//! the menu exist because the document does, and a person in a channel should not
//! have to leave Discord to read what a command takes.

use serde_json::{Map, Value, json};

use super::{CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, CommandError, UPDATE_MESSAGE};
use crate::components::{container, ephemeral, text};
use crate::custom_id::ui::help::Screen;
use crate::docs::{self, discord};
use crate::error::ApiError;
use crate::state::AppState;

/// `Command.handle/4` for `help`: with a name, that command's screen; without
/// one, the list.
///
/// The ids that turn a command's name into a link are read here rather than passed in, and
/// reading them cannot fail: a deployment that cannot answer gets an empty map and the
/// screens say the names alone.
pub async fn command(state: &AppState, options: &Map<String, Value>) -> Value {
    let links = state.links();
    let ids = state.command_ids().await;

    let mut color = COLOR_BRAND;
    let children = match options.get("command").and_then(Value::as_str) {
        Some(name) => match docs::showing_of(name) {
            Some(showing) => discord::command(&showing, links, ids),
            // Autocomplete offers the names that exist, but a name can be typed
            // anyway: the list, with the one sentence that says so above it.
            None => {
                color = COLOR_ERROR;
                let mut children = vec![text(format!(
                    message!("command.help.command.001"),
                    name = name
                ))];
                children.extend(discord::index(links, ids));

                children
            }
        },
        None => discord::index(links, ids),
    };

    screen(children, CHANNEL_MESSAGE_WITH_SOURCE, color)
}

/// `verified/2` for `type` 3: the menu's choice, and the button that goes back.
///
/// Both redraw the message they came from. The screen is one message a person
/// moves through, and a message per choice would leave a trail of them in the
/// channel — which is also why the menu is on the screen it opens.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let picked = crate::custom_id::ui::help::parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    let links = state.links();
    let ids = state.command_ids().await;

    let children = match picked {
        Screen::Select => {
            let chosen = payload
                .get("data")
                .and_then(|data| data.get("values"))
                .and_then(Value::as_array)
                .and_then(|values| values.first())
                .and_then(Value::as_str);

            match chosen.and_then(docs::showing_of) {
                Some(showing) => discord::command(&showing, links, ids),
                // A value no menu of this service offered. The list is where the
                // person already was, so that is the honest screen to leave them
                // on.
                None => discord::index(links, ids),
            }
        }
        Screen::Start => match docs::page_of("start") {
            Some(page) => discord::page(page, links, ids),
            // A page this service does not have is a button nobody drew; the list is where
            // the person already was.
            None => discord::index(links, ids),
        },
        Screen::Index => discord::index(links, ids),
    };

    Ok(screen(children, UPDATE_MESSAGE, COLOR_BRAND))
}

/// A private screen with the normal or error accent selected by the outcome.
fn screen(children: Vec<Value>, kind: i64, color: i64) -> Value {
    json!({
        "type": kind,
        "data": ephemeral(vec![container(Some(color as u32), children)]),
    })
}
