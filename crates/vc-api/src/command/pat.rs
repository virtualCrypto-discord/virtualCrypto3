//! `/pat`: the credentials an account gives to something that is not a browser.
//!
//! Three subcommands, and a token appears exactly once — in the answer to `create`. What the
//! table keeps is the name and the day it expires, so `list` has nothing to leak and `revoke`
//! needs nothing but the name.
//!
//! An addition rather than a port: the Elixir has no personal access token, no API key, and no
//! column that could hold one. `docs/pat.md` is the design.

use serde_json::{Map, Value, json};
use time::{OffsetDateTime, PrimitiveDateTime};

use super::claim::format_date_time;
use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, COLOR_OK, CommandError, get_user,
};
use crate::components::{container, ephemeral, text};
use crate::docs::discord::mentions;
use crate::state::AppState;
use vc_auth::issue::{
    BROWSER_SCOPES, PERSONAL_TTL, PersonalError, personal_token, personal_tokens, revoke_personal,
};

/// The longest a name may be. The option in `scripts/discord-commands.json` states the same
/// bound, and this is the check that makes the statement true.
const NAME_MAX: usize = 32;

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
        "list" => list(state, me).await,
        "revoke" => revoke(state, named(sub_options)?, me).await,
        // Everything this service registers is written down, so a subcommand that is not is one
        // it does not have: a client with a stale command list, answered the way any unknown
        // command is.
        _ => Err(CommandError::Unknown),
    }
}

/// The name a subcommand was given. Both subcommands that take one require it in the registry,
/// so a missing one is a client rather than a person.
fn named(sub_options: Option<&Value>) -> Result<&str, CommandError> {
    sub_options
        .and_then(|options| options.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("pat option name"))
}

/// A token, once: the value, the day it stops working, and what it can do.
///
/// The scopes are read from [`BROWSER_SCOPES`] rather than written out here, because they are the
/// same list the session's token carries and a screen that repeats it by hand is a screen that
/// can disagree with the token it just issued.
async fn create(state: &AppState, name: &str, discord_id: i64) -> Result<Value, CommandError> {
    let Some(account) = account(state, discord_id).await? else {
        return Ok(screen(
            vec![text(
                "VirtualCryptoのアカウントがまだありません。`/create` でアプリケーションを登録すると作られます。",
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
        Ok(token) => {
            let at = PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

            Ok(screen(
                vec![
                    text(format!(
                        "**{name}** としてトークンを発行しました。このトークンは一度だけ表示されます。"
                    )),
                    text(format!("```\n{token}\n```")),
                    text(format!("有効期限: {}", format_date_time(at + PERSONAL_TTL))),
                    text(format!("スコープ: {}", BROWSER_SCOPES.join(", "))),
                ],
                COLOR_OK,
            ))
        }
        // The name is how a token is revoked, so two of them on one account could not be told
        // apart: the second is refused rather than made.
        Err(PersonalError::NameTaken(name)) => Ok(screen(
            vec![text(format!(
                "`{name}` という名前のトークンはもうあります。`/pat revoke` で消してから作ってください。"
            ))],
            COLOR_ERROR,
        )),
        Err(PersonalError::Auth(error)) => Err(error.into()),
    }
}

/// What this account has given out: names and days, never a token.
async fn list(state: &AppState, discord_id: i64) -> Result<Value, CommandError> {
    let Some(account) = account(state, discord_id).await? else {
        return Ok(screen(
            vec![text("VirtualCryptoのアカウントがまだありません。")],
            COLOR_BRAND,
        ));
    };

    let tokens = personal_tokens(state.pool(), i64::from(account)).await?;

    if tokens.is_empty() {
        let ids = state.command_ids().await;

        return Ok(screen(
            vec![text(mentions(
                "まだありません。`/pat create` で作れます。",
                &ids,
            ))],
            COLOR_BRAND,
        ));
    }

    Ok(screen(
        tokens
            .iter()
            .map(|token| {
                text(format!(
                    "**{}**\n有効期限 {}",
                    token.name,
                    format_date_time(token.expires)
                ))
            })
            .collect(),
        COLOR_BRAND,
    ))
}

/// Forgetting one, by the name it was made under. The row is what the token resolves against, so
/// deleting it is the whole of the revocation: the signature stays valid and stops being enough.
async fn revoke(state: &AppState, name: &str, discord_id: i64) -> Result<Value, CommandError> {
    let Some(account) = account(state, discord_id).await? else {
        return Ok(screen(
            vec![text("VirtualCryptoのアカウントがまだありません。")],
            COLOR_ERROR,
        ));
    };

    if !revoke_personal(state.pool(), i64::from(account), name).await? {
        return Ok(screen(
            vec![text(format!("`{name}` という名前のトークンはありません。"))],
            COLOR_ERROR,
        ));
    }

    Ok(screen(
        vec![text(format!("`{name}` を失効させました。"))],
        COLOR_OK,
    ))
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
