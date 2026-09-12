use serde_json::{Map, Value, json};
use vc_core::currency::{CurrencyInfo, CurrencySelector};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, CommandError, EPHEMERAL, as_int,
    get_user, value_text,
};
use crate::state::AppState;

/// `Command.handle/4` for `info`, rendered by `InteractionsJSON.info/1` through
/// `Interactions.Info.render/2`.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let discord_user_id =
        get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let name = options.get("name").map(value_text);
    let unit = options.get("unit").map(value_text);
    let guild_id = payload.get("guild_id").and_then(as_int);

    // `Money.info/1` takes the first selector it is handed, in the order name,
    // unit, guild, id, so a supplied name wins over a supplied unit.
    let selector = match (&name, &unit, guild_id) {
        (Some(name), _, _) => CurrencySelector::Name(name),
        (None, Some(unit), _) => CurrencySelector::Unit(unit),
        (None, None, Some(guild_id)) => CurrencySelector::Guild(guild_id),
        (None, None, None) => {
            return Ok(render_error(
                "DMで実行する場合はオプションを指定する必要があります。",
            ));
        }
    };

    let Some(info) = vc_core::currency::info(state.pool(), selector).await? else {
        return Ok(render_error("通貨が見つかりませんでした。"));
    };

    let guild = match info.guild_id {
        Some(guild_id) => state.discord().get_guild(guild_id).await?,
        None => None,
    };

    let unit = info.unit.clone().unwrap_or_default();
    let balances = vc_core::balance::for_discord_user(state.pool(), discord_user_id).await?;
    let amount = balances
        .iter()
        .find(|balance| balance.unit == unit)
        .map(|balance| balance.amount)
        .unwrap_or(0);

    Ok(render(&info, amount, guild))
}

/// `Interactions.Info.render/2` for `:ok`.
fn render(info: &CurrencyInfo, amount: i64, guild: Option<Map<String, Value>>) -> Value {
    let unit = info.unit.clone().unwrap_or_default();

    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "embeds": [{
                "title": info.name.clone().unwrap_or_default(),
                "author": render_guild(guild),
                "color": COLOR_BRAND,
                "fields": [
                    {
                        "name": "総発行量",
                        "value": format!("`{}{unit}`", info.total_amount),
                        "inline": true,
                    },
                    {
                        "name": "発行枠",
                        "value": format!("`{}{unit}`", info.pool_amount.unwrap_or(0)),
                        "inline": true,
                    },
                    {
                        "name": "あなたの所持量",
                        "value": format!("`{amount}{unit}`"),
                        "inline": true,
                    },
                    {
                        "name": "削除可能",
                        "value": if info.is_deletable(vc_core::model::utc_now()) {
                            "はい"
                        } else {
                            "いいえ"
                        },
                        "inline": true,
                    },
                ],
                "footer": {
                    "text": "発行枠は一日一回総発行量の0.5%増加し、最大で総発行量の3.5%となります。",
                },
            }],
        },
    })
}

/// `Interactions.Info.render/2` for `:error`, which both error cases share.
fn render_error(description: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "embeds": [{
                "title": "エラー",
                "color": COLOR_ERROR,
                "description": description,
            }],
            "allowed_mentions": { "parse": [] },
        },
    })
}

/// `Interactions.Info.render_guild/1`: the guild's name, plus its icon when
/// Discord reports one. A missing icon and an explicit null both add nothing.
fn render_guild(guild: Option<Map<String, Value>>) -> Value {
    let Some(guild) = guild else {
        return Value::Null;
    };

    let mut author = json!({
        "name": guild.get("name").map(value_text).unwrap_or_default(),
    });

    if let Some(Value::String(hash)) = guild.get("icon") {
        let format = if hash.starts_with("a_") {
            "gif"
        } else {
            "webp"
        };
        let id = guild.get("id").map(value_text).unwrap_or_default();

        author["icon_url"] = json!(format!(
            "https://cdn.discordapp.com/icons/{id}/{hash}.{format}"
        ));
    }

    author
}
