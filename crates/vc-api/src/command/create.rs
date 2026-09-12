use serde_json::{Map, Value, json};
use vc_core::currency::{self, CreateError};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_ERROR, COLOR_OK, CommandError, EPHEMERAL, as_int,
    as_permissions, option_text, value_text,
};
use crate::state::AppState;

/// Why a `create` request could not be carried out. The variants mirror the
/// reasons `Command.handle/4` and `Money.create/1` can report.
enum Reason {
    Guild,
    Name,
    Unit,
    Invalid,
    Permission,
    InvalidAmount,
    RunInDm,
}

impl Reason {
    /// `Interactions.Create.render_error/2`.
    fn message(&self, options: &Map<String, Value>) -> String {
        match self {
            Reason::Guild => "このギルドではすでに通貨が作成されています。".to_string(),
            Reason::Name => format!(
                "`{}`という名前の通貨は存在しています。別の名前を使用してください。",
                option_display(options, "name"),
            ),
            Reason::Unit => format!(
                "`{}`という単位の通貨は存在しています。別の単位を使用してください。",
                option_display(options, "unit"),
            ),
            Reason::Invalid => {
                "通貨の名前は2から16文字以内の英数字、単位は1から10文字以内の英小文字を使ってください。"
                    .to_string()
            }
            Reason::Permission => "実行には管理者権限が必要です。".to_string(),
            Reason::InvalidAmount => {
                "不正な金額です。1以上4294967295以下である必要があります。".to_string()
            }
            Reason::RunInDm => "DMでは実行できません。".to_string(),
        }
    }
}

/// `Command.handle/4` for `create`, rendered by `InteractionsJSON.create/1`
/// through `Interactions.Create.render/3`.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let member = payload.get("member");

    let guild_id = payload.get("guild_id").and_then(as_int);
    let user_id = member
        .and_then(|member| member.get("user"))
        .and_then(|user| user.get("id"))
        .and_then(as_int);

    // `handle/4` only has a guild clause, so a direct message has no handler.
    let (Some(guild_id), Some(user_id)) = (guild_id, user_id) else {
        return Ok(render_error(Reason::RunInDm, options));
    };

    let permissions = member
        .and_then(|member| member.get("permissions"))
        .and_then(as_permissions)
        .ok_or_else(|| CommandError::missing("create has no permissions"))?;

    let amount = options
        .get("amount")
        .and_then(as_int)
        .ok_or_else(|| CommandError::missing("create has no amount"))?;

    let guild = state.discord().get_guild(guild_id).await?;
    if !may_manage(guild.as_ref(), permissions) {
        return Ok(render_error(Reason::Permission, options));
    }

    let name = option_text(options, "name")?;
    let unit = option_text(options, "unit")?;

    if !name_unit_check(&name, &unit) {
        return Ok(render_error(Reason::Invalid, options));
    }

    match currency::create(state.pool(), guild_id, &name, &unit, user_id, amount).await {
        Ok(()) => Ok(render_ok(options)),
        Err(CreateError::Guild) => Ok(render_error(Reason::Guild, options)),
        Err(CreateError::Unit) => Ok(render_error(Reason::Unit, options)),
        Err(CreateError::Name) => Ok(render_error(Reason::Name, options)),
        Err(CreateError::InvalidAmount) => Ok(render_error(Reason::InvalidAmount, options)),
        Err(CreateError::Database(error)) => Err(CommandError::from(error)),
    }
}

/// `Interactions.Create.render/3` for `{:ok, :ok, options}`.
fn render_ok(options: &Map<String, Value>) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "embeds": [{
                "description": format!(
                    "\u{2705} 通貨の作成に成功しました！ `/info unit: {}`コマンドで通貨の情報をご覧ください。\n\
                     削除したい場合は、72時間以内に`/delete`コマンドを実行してください。",
                    option_display(options, "unit"),
                ),
                "color": COLOR_OK,
            }],
            "allowed_mentions": { "parse": [] },
        },
    })
}

/// `Interactions.Create.render/3` for `{:error, reason, options}`.
fn render_error(reason: Reason, options: &Map<String, Value>) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "embeds": [{
                "title": "エラー",
                "description": reason.message(options),
                "color": COLOR_ERROR,
            }],
            "allowed_mentions": { "parse": [] },
        },
    })
}

fn option_display(options: &Map<String, Value>, name: &str) -> String {
    options.get(name).map(value_text).unwrap_or_default()
}

/// `Command.continue_management_command?/2`: a guild that has moved to
/// application-command permissions wants the administrator bit, and every other
/// guild is allowed through.
fn may_manage(guild: Option<&Map<String, Value>>, permissions: u64) -> bool {
    const ADMINISTRATOR: u64 = 0x8;

    let requires_v2 = guild
        .and_then(|guild| guild.get("features"))
        .and_then(Value::as_array)
        .is_some_and(|features| {
            features
                .iter()
                .any(|feature| feature == "APPLICATION_COMMAND_PERMISSIONS_V2")
        });

    !requires_v2 || permissions & ADMINISTRATOR == ADMINISTRATOR
}

/// `Command.name_unit_check/2`. Both regexes match anywhere in the string rather
/// than being anchored, so a name needs a run of two alphanumerics and a unit
/// needs one lowercase letter.
fn name_unit_check(name: &str, unit: &str) -> bool {
    has_alphanumeric_run(name, 2) && unit.chars().any(|character| character.is_ascii_lowercase())
}

fn has_alphanumeric_run(value: &str, length: usize) -> bool {
    let mut run = 0;

    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            run += 1;

            if run >= length {
                return true;
            }
        } else {
            run = 0;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::name_unit_check;

    #[test]
    fn a_name_needs_two_alphanumerics_and_a_unit_one_lowercase_letter() {
        assert!(name_unit_check("funyua", "a"));
        assert!(name_unit_check("nyan123", "unit"));

        assert!(!name_unit_check(" ", "a"));
        assert!(!name_unit_check("a", "a"));
        assert!(!name_unit_check("funyua", "AA"));
    }
}
