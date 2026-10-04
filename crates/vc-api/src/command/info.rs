use serde_json::{Map, Value, json};
use vc_core::currency::{CurrencyInfo, CurrencySelector};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, CommandError, as_int, error_text,
    get_user, money_text, unit_text, value_text,
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
            return Ok(render_error(message!("command.info.handle.001")));
        }
    };

    let Some(info) = vc_core::currency::info(state.pool(), selector).await? else {
        return Ok(render_error(message!("command.info.handle.002")));
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
///
/// The embed this replaces had the guild as its **author**, the currency as its **title**, and
/// four **fields** under them. What a person wants first is which currency this is, so the name
/// is the title, the guild sits under it with its icon as the section's accessory, where the
/// author's small icon used to sit, and the fields are `label: value` lines, the way the other
/// screens list what they show.
fn render(info: &CurrencyInfo, amount: i64, guild: Option<Map<String, Value>>) -> Value {
    let unit = info.unit.clone().unwrap_or_default();

    let mut children = Vec::new();

    children.push(crate::components::text(format!(
        message!("command.info.render.001"),
        info.name.clone().unwrap_or_default()
    )));

    // The guild name is a line of its own, and its icon is the accessory that keeps it from being a
    // bare sentence: a section is the pairing, and a section needs one — so an icon that is not
    // there leaves a Text Display.
    if let Some(guild) = &guild {
        let name = guild.get("name").map(value_text).unwrap_or_default();

        children.push(match guild_icon(guild) {
            Some(url) => crate::components::section(
                vec![crate::components::text(format!(
                    message!("command.info.render.002"),
                    name = name
                ))],
                crate::components::thumbnail(&url),
            ),
            None => {
                crate::components::text(format!(message!("command.info.render.003"), name = name))
            }
        });
    }

    // The fields were `inline`, four columns in an embed. A Text Display is a block, so they are
    // one line each in a single block, as the other screens write theirs.
    let fields = [
        format!(message!("command.info.render.004"), unit = unit_text(&unit)),
        format!(
            message!("command.info.render.005"),
            money_text(info.total_amount, &unit)
        ),
        format!(
            message!("command.info.render.006"),
            money_text(info.pool_amount.unwrap_or(0), &unit)
        ),
        format!(
            message!("command.info.render.007"),
            money_text(amount, &unit)
        ),
        format!(
            message!("command.info.render.008"),
            if info.is_deletable(vc_core::model::utc_now()) {
                message!("command.info.render.009")
            } else {
                message!("command.info.render.010")
            }
        ),
    ];
    children.push(crate::components::text(fields.join("\n")));
    // `-# ` is how a line is written small, which is what the footer was.
    children.push(crate::components::text(message!("command.info.render.011")));

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
                vec![crate::components::text(error_text(description))],
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
