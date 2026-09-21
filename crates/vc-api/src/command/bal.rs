use serde_json::{Value, json};

use super::{CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, CommandError};
use crate::components::text;
use crate::state::AppState;

/// `Command.handle/4` for `bal`, rendered by `InteractionsJSON.bal/1` through
/// `Interactions.Bal.render/1`.
pub async fn handle(state: &AppState, payload: &Value) -> Result<Value, CommandError> {
    let discord_user_id =
        super::get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let balances = vc_core::balance::for_discord_user(state.pool(), discord_user_id).await?;

    Ok(render(&balances))
}

/// `Interactions.Bal.render_content/1`'s content, in the shape the screens beside it use.
///
/// The Elixir answered with one `yaml` fence holding a line per currency, and that is what
/// this did first as well. What each line says is unchanged — the currency, the amount and
/// the unit, in unit order — and the sentence for an empty list is the Elixir's own, word
/// for word. The fence is gone because a fence is not a screen: the lists this service grew
/// afterwards are containers with a header and a line per row, and a person's balances
/// belong in the same shape.
fn render(balances: &[vc_core::balance::Balance]) -> Value {
    let mut children = Vec::new();

    if balances.is_empty() {
        children.push(text("通貨を持っていません。"));
    } else {
        children.push(text(format!("**所持通貨一覧** ({}件)", balances.len())));

        for balance in balances {
            children.push(text(format!(
                "**{}**\n{} {}",
                balance.name, balance.amount, balance.unit
            )));
        }
    }

    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": crate::components::EPHEMERAL | crate::components::IS_COMPONENTS_V2,
            "components": [crate::components::container(
                Some(COLOR_BRAND as u32),
                children,
            )],
        },
    })
}
