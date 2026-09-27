//! `/pat`: the credentials an account gives to something that is not a browser.
//!
//! `create` shows the credential once, privately. `list` shows names with revocation buttons.
//! Ten rows per page leave room for the controls within Discord's component limit.
//!
//! An addition rather than a port: the Elixir has no personal access token, no API key, and no
//! column that could hold one. `docs/pat.md` is the design.

use serde_json::{Map, Value, json};
use time::OffsetDateTime;

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, COLOR_OK, CommandError, UPDATE_MESSAGE,
    get_user,
};
use crate::components::{
    ButtonStyle, action_row, button, container, ephemeral, icon_button, section, text,
};
use crate::custom_id::ui::pat::{self as ids, Pressed};
use crate::docs::discord::mentions;
use crate::state::AppState;
use vc_auth::issue::{
    BROWSER_SCOPES, MAX_PERSONAL_TOKENS as MAX_TOKENS, PersonalError, personal_token,
    personal_tokens, revoke_personal,
};

/// The longest a name may be. `crate::discord_commands`'s `/pat name` option states the same
/// bound — one number in the option Discord shows and in the check that makes it true.
const NAME_MAX: usize = 32;
const PER_PAGE: usize = 10;

/// `Command.handle/4` for `pat`.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let subcommand = options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("pat has no subcommand"))?;

    let sub_options = options.get("sub_options");

    match subcommand {
        "create" => create(state, named(sub_options)?, me).await,
        "list" => list(state, me, 1, None, CHANNEL_MESSAGE_WITH_SOURCE).await,
        // Everything this service registers is written down, so a subcommand that is not is one
        // it does not have: a client with a stale command list, answered the way any unknown
        // command is.
        _ => Err(CommandError::Unknown),
    }
}

/// The required name for creation.
fn named(sub_options: Option<&Value>) -> Result<&str, CommandError> {
    sub_options
        .and_then(|options| options.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("pat option name"))
}

/// A token, once: the value, and what it can do.
///
/// The scopes come from the issuance constant so they agree with the credential.
async fn create(state: &AppState, name: &str, discord_id: i64) -> Result<Value, CommandError> {
    let Some(account) = account(state, discord_id).await? else {
        return Ok(screen(
            vec![text(
                "VirtualCryptoのアカウントがまだありません。`/application register` でアプリケーションを登録すると作られます。",
            )],
            COLOR_ERROR,
        ));
    };

    let length = name.chars().count();

    if length == 0 || length > NAME_MAX {
        return Ok(screen(
            vec![text(format!("名前は1〜{NAME_MAX}文字です。"))],
            COLOR_ERROR,
        ));
    }

    let now = OffsetDateTime::now_utc();

    match personal_token(
        state.pool(),
        state.jwt_secret(),
        i64::from(account),
        name,
        now,
    )
    .await
    {
        Ok(token) => Ok(screen(
            vec![
                text(format!(
                    "{} としてトークンを発行しました。値は実行した本人だけに、この返信で一度だけ表示されます。",
                    display_name(name)
                )),
                text(format!("```\n{token}\n```")),
                text(
                    "このトークンを持つツールは、あなたとしてアプリケーションの登録と接続、残高の参照、送金、請求、契約の承認などができます。権限は絞れません。他人に見せないでください。",
                ),
                text(format!("スコープ: {}", BROWSER_SCOPES.join(", "))),
                text(mentions(
                    "有効期限はありません。`/pat list` の名前の横にある「失効」ボタンで失効させるまで使えます。",
                    state.command_ids().await,
                )),
            ],
            COLOR_OK,
        )),
        Err(PersonalError::NameTaken(name)) => Ok(screen(
            vec![text(mentions(
                &format!(
                    "{} という名前のトークンはもうあります。別の名前を使うか、`/pat list` の「失効」ボタンで失効させてから作ってください。",
                    display_name(&name)
                ),
                state.command_ids().await,
            ))],
            COLOR_ERROR,
        )),
        Err(PersonalError::LimitReached) => Ok(screen(
            vec![text(mentions(
                &format!(
                    "トークンは1アカウントに{MAX_TOKENS}個までです。`/pat list` で不要なトークンの\
                     「失効」ボタンを押してから作ってください。"
                ),
                state.command_ids().await,
            ))],
            COLOR_ERROR,
        )),
        Err(PersonalError::Auth(error)) => Err(error.into()),
    }
}

/// Names and actions only: credential values cannot be read back.
async fn list(
    state: &AppState,
    discord_id: i64,
    number: usize,
    notice: Option<(String, i64)>,
    kind: i64,
) -> Result<Value, CommandError> {
    let Some(account) = account(state, discord_id).await? else {
        return Ok(screen(
            vec![text("VirtualCryptoのアカウントがまだありません。")],
            COLOR_BRAND,
        ));
    };

    let tokens = personal_tokens(state.pool(), i64::from(account)).await?;

    let mut children = Vec::new();
    let mut color = COLOR_BRAND;
    if let Some((message, accent)) = notice {
        children.push(text(message));
        color = accent;
    }
    if tokens.is_empty() {
        children.push(text(mentions(
            "まだありません。`/pat create` で作れます。",
            state.command_ids().await,
        )));
    } else {
        let last = tokens.len().div_ceil(PER_PAGE);
        let number = number.clamp(1, last);
        children.push(text(format!(
            "**個人アクセストークン** ({} / {MAX_TOKENS}個・{number} / {last}ページ)\n有効期限はありません。「失効」を押すと直ちに使えなくなり、そのトークンを使うツールも動かなくなります。",
            tokens.len()
        )));
        for token in tokens.iter().skip((number - 1) * PER_PAGE).take(PER_PAGE) {
            children.push(section(
                vec![text(display_name(&token.name))],
                button(
                    &ids::revoke(discord_id, number, token.token_id),
                    "失効",
                    ButtonStyle::Danger,
                ),
            ));
        }
        if last > 1 {
            let previous = icon_button(
                &ids::page(discord_id, number - 1),
                "⏮️",
                ButtonStyle::Secondary,
                Some(number == 1),
            );
            let next = icon_button(
                &ids::page(discord_id, number + 1),
                "⏭️",
                ButtonStyle::Secondary,
                Some(number == last),
            );
            children.push(action_row(vec![previous, next]));
        }
    }
    Ok(json!({
        "type": kind,
        "data": ephemeral(vec![container(Some(color as u32), children)]),
    }))
}

/// A button never trusts its embedded owner or token id as authorization.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let discord_id =
        get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let pressed =
        ids::parse(&crate::custom_id::parse(custom_id)).map_err(|_| CommandError::Unknown)?;
    let (Pressed::Page { owner, number } | Pressed::Revoke { owner, number, .. }) = pressed;
    if owner != discord_id {
        return Ok(screen(
            vec![text(
                "この一覧は操作できません。`/pat list` で自分の一覧を開いてください。",
            )],
            COLOR_ERROR,
        ));
    }
    let Pressed::Revoke { token_id, .. } = pressed else {
        return list(state, discord_id, number, None, UPDATE_MESSAGE).await;
    };
    let Some(account) = account(state, discord_id).await? else {
        return Ok(screen(
            vec![text("VirtualCryptoのアカウントがまだありません。")],
            COLOR_ERROR,
        ));
    };

    let notice = match revoke_personal(state.pool(), i64::from(account), token_id).await? {
        Some(name) => (
            format!("{} を失効させました。", display_name(&name)),
            COLOR_OK,
        ),
        None => (
            "このトークンは既に失効しているか、見つかりません。".to_owned(),
            COLOR_ERROR,
        ),
    };
    list(state, discord_id, number, Some(notice), UPDATE_MESSAGE).await
}

/// Keep a user-chosen name from hiding or reformatting the adjacent action.
fn display_name(name: &str) -> String {
    let mut escaped = String::new();
    for c in name.chars() {
        if "\\`*_{}[]()<>#+-.!|~".contains(c) {
            escaped.push('\\');
        }
        escaped.push(if c == '\n' || c == '\r' { ' ' } else { c });
    }
    format!("**{escaped}**")
}

/// The account behind an interaction, or nothing when the caller has none yet — a token belongs
/// to an account, so an account has to exist before one can be made.
async fn account(state: &AppState, discord_id: i64) -> Result<Option<i32>, CommandError> {
    Ok(vc_core::user::find_by_discord_id(state.pool(), discord_id)
        .await?
        .map(|user| user.id))
}

/// An ephemeral screen in one colour, which is the shape every answer here has: two of them
/// carry or name a credential, so nobody else in the channel sees any of them.
fn screen(children: Vec<Value>, color: i64) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": ephemeral(vec![container(Some(color as u32), children)]),
    })
}
