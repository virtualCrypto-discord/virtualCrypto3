use serde_json::{Value, json};

use super::{CHANNEL_MESSAGE_WITH_SOURCE, CommandError, EPHEMERAL};
use crate::state::AppState;

/// `Command.handle/4` for `bal`, rendered by `InteractionsJSON.bal/1` through
/// `Interactions.Bal.render/1`.
pub async fn handle(state: &AppState, payload: &Value) -> Result<Value, CommandError> {
    let discord_user_id =
        super::get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let balances = vc_core::balance::for_discord_user(state.pool(), discord_user_id).await?;

    Ok(render(&balances))
}

/// `Interactions.Bal.render_content/1`: a `yaml` fence listing the currencies,
/// or a plain fence when there are none.
fn render(balances: &[vc_core::balance::Balance]) -> Value {
    let content = if balances.is_empty() {
        "所持通貨一覧\n```\n通貨を持っていません。\n```".to_string()
    } else {
        let lines = balances
            .iter()
            .map(|balance| format!("{}: {} {}", balance.name, balance.amount, balance.unit))
            .collect::<Vec<_>>()
            .join("\n");

        format!("所持通貨一覧\n```yaml\n{lines}\n```")
    };

    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "content": content,
        },
    })
}
