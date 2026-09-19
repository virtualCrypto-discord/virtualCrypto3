pub mod action;
pub mod button;
pub mod list;
pub mod make;
pub mod show;

use serde_json::{Map, Value, json};
use time::PrimitiveDateTime;
use vc_core::claim::Transition;

use crate::claim_list::Position;

use super::{CHANNEL_MESSAGE_WITH_SOURCE, COLOR_ERROR, CommandError};
use crate::state::AppState;

/// `Command.handle/4`'s `"claim"` clauses: the subcommand picks a handler.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let subcommand = options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("claim has no subcommand"))?;

    let sub_options = options.get("sub_options");

    match subcommand {
        "approve" => action::handle(state, sub_options, payload, Transition::Approved).await,
        "cancel" => action::handle(state, sub_options, payload, Transition::Canceled).await,
        "deny" => action::handle(state, sub_options, payload, Transition::Denied).await,
        "make" => make::handle(state, sub_options, payload).await,
        "list" => list::handle(state, sub_options, payload, Position::All).await,
        "received" => list::handle(state, sub_options, payload, Position::Received).await,
        "sent" => list::handle(state, sub_options, payload, Position::Claimed).await,
        "show" => show::handle(state, sub_options, payload).await,
        _ => Err(CommandError::Unknown),
    }
}

/// An option inside `sub_options`. Elixir indexes the nested map directly, so a
/// missing one is a raise rather than an answer.
fn sub_option<'a>(sub_options: Option<&'a Value>, name: &str) -> Result<&'a Value, CommandError> {
    sub_options
        .and_then(|options| options.get(name))
        .ok_or_else(|| CommandError::missing(&format!("claim option {name}")))
}

/// `Interactions.Util.format_date_time/1`: Discord renders `<t:unix>` in the
/// reader's own timezone.
///
/// Shared with the contract screen, which has a deadline to say the same way and
/// would otherwise have a second copy of this format.
pub(super) fn format_date_time(value: PrimitiveDateTime) -> String {
    format!("<t:{}>", value.assume_utc().unix_timestamp())
}

/// `Interactions.Claim.render/1` for `{:error, _, error}`.
///
/// The title is a line of its own, as an embed's title was: components have no title, so it is
/// bold text above the sentence it introduced.
fn render_error(description: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            Some(COLOR_ERROR as u32),
            vec![crate::components::text(format!("**エラー**\n{description}"))],
        )]),
    })
}
