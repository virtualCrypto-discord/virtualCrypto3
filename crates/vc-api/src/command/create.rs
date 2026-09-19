use serde_json::{Map, Value, json};
use vc_core::currency::{self, CreateError};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_ERROR, COLOR_OK, CommandError, as_int, as_permissions,
    is_administrator, option_text, value_text,
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

    if !is_administrator(permissions) {
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
        "data": crate::components::ephemeral(vec![crate::components::container(
            // The accent the embed carried.
            Some(COLOR_OK as u32),
            vec![crate::components::text(format!(
                "\u{2705} 通貨の作成に成功しました！ `/info unit: {}`コマンドで通貨の情報をご覧ください。\n\
                 削除したい場合は、72時間以内に`/delete`コマンドを実行してください。",
                option_display(options, "unit"),
            ))],
        )]),
    })
}

/// `Interactions.Create.render/3` for `{:error, reason, options}`.
fn render_error(reason: Reason, options: &Map<String, Value>) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": crate::components::ephemeral(vec![crate::components::container(
            Some(COLOR_ERROR as u32),
            // The title a line of its own, as the embed had it.
            vec![crate::components::text(format!(
                "**エラー**\n{}",
                reason.message(options)
            ))],
        )]),
    })
}

fn option_display(options: &Map<String, Value>, name: &str) -> String {
    options.get(name).map(value_text).unwrap_or_default()
}

/// `Command.name_unit_check/2`: what a currency's name and unit may be, which is
/// the sentence a refusal carries — two to sixteen alphanumerics, and one to ten
/// lowercase letters.
///
/// The Elixir's regexes were unanchored, so they matched anywhere in the string:
/// `funyua!` was a name, a name of forty characters was one too, and `u1` was a
/// unit. The sentence is what somebody reads before typing one, so the sentence
/// is what is checked.
fn name_unit_check(name: &str, unit: &str) -> bool {
    is_name(name) && is_unit(unit)
}

/// Two to sixteen characters, every one of them a letter or a digit.
fn is_name(name: &str) -> bool {
    let length = name.chars().count();

    (2..=16).contains(&length)
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
}

/// One to ten characters, every one of them a lowercase letter.
fn is_unit(unit: &str) -> bool {
    let length = unit.chars().count();

    (1..=10).contains(&length) && unit.chars().all(|character| character.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::name_unit_check;

    #[test]
    fn a_name_is_two_to_sixteen_alphanumerics_and_a_unit_one_to_ten_lowercase_letters() {
        assert!(name_unit_check("funyua", "a"));
        assert!(name_unit_check("nyan123", "unit"));
        assert!(name_unit_check("ab", "a"), "the shortest name");
        assert!(name_unit_check("0123456789abcdef", "a"), "the longest name");
        assert!(name_unit_check("funyua", "abcdefghij"), "the longest unit");

        assert!(!name_unit_check(" ", "a"));
        assert!(!name_unit_check("a", "a"), "one character is too short");
        assert!(
            !name_unit_check("0123456789abcdefg", "a"),
            "seventeen is too long"
        );
        assert!(
            !name_unit_check("funyua!", "a"),
            "a mark is not an alphanumeric"
        );
        assert!(!name_unit_check("ふにゅあ", "a"), "and neither is kana");

        assert!(!name_unit_check("funyua", "AA"));
        assert!(!name_unit_check("funyua", ""), "a unit needs a letter");
        assert!(
            !name_unit_check("funyua", "u1"),
            "a digit is not a lowercase letter"
        );
        assert!(
            !name_unit_check("funyua", "abcdefghijk"),
            "eleven is too long"
        );
    }
}
