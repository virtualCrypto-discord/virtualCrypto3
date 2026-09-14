use serde_json::{Map, Value, json};
use vc_core::currency::{CurrencyInfo, CurrencySelector};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, CommandError, as_int, get_user,
    value_text,
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

    let mut children = Vec::new();

    // The guild was the embed's author: a line with a small icon beside it. A section is that
    // pairing — its text with an accessory — and a thumbnail is the small image, so the icon keeps
    // its place and moves to the right of the sentence. With no icon there is no accessory to
    // have, and a section requires one, so the name is a Text Display of its own.
    if let Some(guild) = &guild {
        let name = guild.get("name").map(value_text).unwrap_or_default();

        children.push(match guild_icon(guild) {
            Some(url) => crate::components::section(
                vec![crate::components::text(format!("**{name}**"))],
                crate::components::thumbnail(&url),
            ),
            None => crate::components::text(format!("**{name}**")),
        });
    }

    // The title, then the four fields, then the footer. The fields were `inline`, which is four
    // columns in an embed and four lines here: a Text Display is a block, so the arrangement is
    // the one thing about this message that components cannot say the same way.
    children.push(crate::components::text(format!(
        "**{}**",
        info.name.clone().unwrap_or_default()
    )));
    children.push(crate::components::text(format!(
        "**総発行量**\n`{}{unit}`",
        info.total_amount
    )));
    children.push(crate::components::text(format!(
        "**発行枠**\n`{}{unit}`",
        info.pool_amount.unwrap_or(0)
    )));
    children.push(crate::components::text(format!(
        "**あなたの所持量**\n`{amount}{unit}`"
    )));
    children.push(crate::components::text(format!(
        "**削除可能**\n{}",
        if info.is_deletable(vc_core::model::utc_now()) {
            "はい"
        } else {
            "いいえ"
        }
    )));
    // `-# ` is how a line is written small, which is what the footer was.
    children.push(crate::components::text(
        "-# 発行枠は一日一回総発行量の0.5%増加し、最大で総発行量の3.5%となります。",
    ));

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

/// `Interactions.Info.render/2` for `:error`, which both error cases share.
fn render_error(description: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": crate::components::EPHEMERAL | crate::components::IS_COMPONENTS_V2,
            "components": [crate::components::container(
                Some(COLOR_ERROR as u32),
                vec![crate::components::text(format!("**エラー**\n{description}"))],
            )],
            "allowed_mentions": { "parse": [] },
        },
    })
}

/// `Interactions.Info.render_guild/1`, as the icon alone.
///
/// A missing icon and an explicit null both add nothing, which is why this is an `Option`: the
/// section that shows the guild's name needs an accessory, and a URL that is not one would be
/// worse than having none.
fn guild_icon(guild: &Map<String, Value>) -> Option<String> {
    let Some(Value::String(hash)) = guild.get("icon") else {
        return None;
    };

    let format = if hash.starts_with("a_") {
        "gif"
    } else {
        "webp"
    };
    let id = guild.get("id").map(value_text).unwrap_or_default();

    Some(format!(
        "https://cdn.discordapp.com/icons/{id}/{hash}.{format}"
    ))
}
