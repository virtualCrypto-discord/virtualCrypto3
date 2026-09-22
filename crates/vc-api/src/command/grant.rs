//! Common grant approval and separate personal/server grant management.
//! Every button rechecks the interaction actor and the request's live state.

use serde_json::{Map, Value, json};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, COLOR_OK, CommandError, UPDATE_MESSAGE,
    as_int, as_permissions, get_user, is_administrator, value_text,
};
use crate::components::{ButtonStyle, action_row, button, container, ephemeral, icon_button, text};
use crate::custom_id::ui::grant::{Page, Pressed, page_custom_id, revoke_custom_id};
use crate::error::ApiError;
use crate::state::AppState;
use vc_core::grant::Target;

/// How many applications one screen shows, for the contract screen's reason: five
/// fits in one message without scrolling it off the screen, and a sixth waits for
/// the screen the next revoke redraws.
const MAX_APPLICATIONS: usize = 5;

/// Where the screen starts, and where a revoke draws next: page numbers count from
/// one, as the claim list's do.
const FIRST_PAGE: i64 = 1;

pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let user = actor(payload)?;
    let subcommand = options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("grant has no subcommand"))?;
    match subcommand {
        "approve" => {
            let code = options
                .get("sub_options")
                .and_then(|options| options.get("code"))
                .map(value_text)
                .ok_or_else(|| CommandError::missing("grant approve has no code"))?;
            let reviews = vc_core::grant::review_requests(
                state.pool(),
                Some(code.trim()),
                None,
                user,
                authorized_guild(payload),
                time::OffsetDateTime::now_utc(),
            )
            .await?;
            let Some(review) = reviews.first() else {
                return Ok(render_error(NOT_FOUND));
            };
            // Global pending-code uniqueness makes this exactly one request.
            if reviews.len() != 1 || !valid_scopes(review) {
                return Ok(render_error(NOT_FOUND));
            }
            let units = crate::resource::units(state.pool(), &review.resources).await?;
            let target = match review.target {
                Target::User(id) => format!(
                    "Your account ({id})\nApproval replaces this application's previous personal permissions."
                ),
                Target::Guild(id) => format!("This server ({id})"),
            };
            Ok(answered(
                container(
                    Some(COLOR_BRAND as u32),
                    vec![
                        text(format!(
                            "**Review application access**\nApplication: {}\nClient: `{}`\nTarget: {}\n\n{}\nCurrencies: {}",
                            name_of(review.client_name.as_deref()),
                            review.client_id,
                            target,
                            permissions(&review.scopes),
                            currencies(&units)
                        )),
                        action_row(vec![button(
                            &crate::custom_id::ui::grant::confirm_custom_id(review.request_id),
                            "Approve",
                            ButtonStyle::Success,
                        )]),
                    ],
                ),
                CHANNEL_MESSAGE_WITH_SOURCE,
            ))
        }
        "user" => Ok(answered(
            personal_page(state, user, FIRST_PAGE).await?,
            CHANNEL_MESSAGE_WITH_SOURCE,
        )),
        // Keep already-issued /grant list interactions as a server-list alias.
        "server" | "list" => {
            if payload.get("guild_id").and_then(as_int).is_none() {
                return Ok(render_error("エラー: DMでは実行できません。"));
            }
            let Some(guild) = authorized_guild(payload) else {
                return Ok(render_error("エラー: 実行には管理者権限が必要です。"));
            };
            Ok(answered(
                page(state, guild, FIRST_PAGE).await?,
                CHANNEL_MESSAGE_WITH_SOURCE,
            ))
        }
        _ => Err(CommandError::Unknown),
    }
}

const NOT_FOUND: &str = "No pending request is available for you here. Server requests require an administrator in that server.";

fn actor(payload: &Value) -> Result<i64, CommandError> {
    get_user(payload).ok_or_else(|| CommandError::missing("grant has no user"))
}

fn authorized_guild(payload: &Value) -> Option<i64> {
    let guild = payload.get("guild_id").and_then(as_int)?;
    let permissions = payload
        .get("member")?
        .get("permissions")
        .and_then(as_permissions)?;
    is_administrator(permissions).then_some(guild)
}

fn valid_scopes(review: &vc_core::grant::Review) -> bool {
    match review.target {
        Target::Guild(_) => vc_core::application::check_scopes(&review.scopes).is_ok(),
        Target::User(_) => vc_core::application::check_personal_scopes(&review.scopes).is_ok(),
    }
}

pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let user = actor(payload)?;
    let pressed = crate::custom_id::ui::grant::parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;
    match pressed {
        Pressed::Confirmed(request) => {
            let reviews = vc_core::grant::review_requests(
                state.pool(),
                None,
                Some(request),
                user,
                authorized_guild(payload),
                time::OffsetDateTime::now_utc(),
            )
            .await?;
            let Some(review) = reviews.first().filter(|review| valid_scopes(review)) else {
                return Ok(error_screen(NOT_FOUND));
            };
            if !vc_core::grant::approve_review(
                state.pool(),
                review,
                time::OffsetDateTime::now_utc(),
            )
            .await?
            {
                return Ok(error_screen(NOT_FOUND));
            }
            match review.target {
                Target::Guild(guild) => state
                    .notifier()
                    .notify_grant_decided(review.application_id, guild),
                Target::User(user) => state.notifier().notify_personal_grant_decided(
                    review.application_id,
                    user,
                    &review.scopes,
                ),
            }
            let units = crate::resource::units(state.pool(), &review.resources).await?;
            let message = match review.target {
                Target::Guild(_) => format!(
                    "発行を許可しました。この申請は、{}を操作できます。",
                    currencies(&units)
                ),
                Target::User(_) => format!(
                    "Access approved.\n{}\nCurrencies: {}",
                    permissions(&review.scopes),
                    currencies(&units)
                ),
            };
            Ok(answered(
                container(Some(COLOR_OK as u32), vec![text(message)]),
                UPDATE_MESSAGE,
            ))
        }
        Pressed::UserPaged(owner, number) => {
            if owner != user {
                return Ok(error_screen("This is another user's grant list."));
            }
            Ok(answered(
                personal_page(state, user, number).await?,
                UPDATE_MESSAGE,
            ))
        }
        Pressed::UserRevoked(owner, client) => {
            if owner != user {
                return Ok(error_screen("This is another user's grant list."));
            }
            if let Some(app) = vc_core::grant::revoke_personal(state.pool(), client, user).await? {
                state
                    .notifier()
                    .notify_personal_grant_decided(app, user, &[]);
            }
            Ok(answered(
                personal_page(state, user, FIRST_PAGE).await?,
                UPDATE_MESSAGE,
            ))
        }
        Pressed::Paged(_, number) => {
            let Some(guild) = authorized_guild(payload) else {
                return Ok(error_screen("実行には管理者権限が必要です。"));
            };
            Ok(answered(page(state, guild, number).await?, UPDATE_MESSAGE))
        }
        Pressed::Revoked(client) => {
            let Some(guild) = authorized_guild(payload) else {
                return Ok(error_screen("実行には管理者権限が必要です。"));
            };
            if let Some(app) = vc_core::grant::revoke_grant(state.pool(), &client, guild).await? {
                state.notifier().notify_grant_decided(app, guild);
            }
            Ok(answered(
                page(state, guild, FIRST_PAGE).await?,
                UPDATE_MESSAGE,
            ))
        }
    }
}

fn permissions(scopes: &[String]) -> String {
    if scopes.is_empty() {
        return "No operations permitted.".to_owned();
    }
    scopes
        .iter()
        .map(|scope| {
            let description = if scope == "vc.issue" {
                "Issue currency from this server's pool"
            } else {
                vc_core::delegation::Scope::parse(scope)
                    .map(|scope| scope.description())
                    .unwrap_or("Unsupported permission; request a new approval")
            };
            format!("- {description} (`{scope}`)")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn personal_page(state: &AppState, user: i64, requested: i64) -> Result<Value, CommandError> {
    // Scope descriptions need more space than the guild's single issuing scope.
    const LIMIT: i64 = 2;
    let (apps, total, page) =
        vc_core::grant::personal_applications(state.pool(), user, requested, LIMIT).await?;
    let last = ((total + LIMIT - 1) / LIMIT).max(1);
    let mut children = vec![text(format!(
        "**Applications with access to your account** ({total})\nPage {page}/{last}"
    ))];
    if apps.is_empty() {
        children.push(text(
            "No applications have access. Use `/grant approve code:` to review a request.",
        ));
    }
    for app in apps {
        let units = crate::resource::units(state.pool(), &app.resources).await?;
        children.push(text(format!(
            "**{}**\n`{}`\n{}\nCurrencies: {}",
            name_of(app.client_name.as_deref()),
            app.client_id,
            permissions(&app.scopes),
            currencies(&units)
        )));
        children.push(action_row(vec![button(
            &crate::custom_id::ui::grant::user_revoke_custom_id(user, &app.client_id),
            "Revoke",
            ButtonStyle::Danger,
        )]));
    }
    if last > 1 {
        children.push(action_row(vec![
            icon_button(
                &crate::custom_id::ui::grant::user_page_custom_id(user, 1),
                "⏪",
                ButtonStyle::Secondary,
                Some(page == 1),
            ),
            icon_button(
                &crate::custom_id::ui::grant::user_page_custom_id(user, (page - 1).max(1)),
                "⏮️",
                ButtonStyle::Secondary,
                Some(page == 1),
            ),
            icon_button(
                &crate::custom_id::ui::grant::user_page_custom_id(user, (page + 1).min(last)),
                "⏭️",
                ButtonStyle::Secondary,
                Some(page == last),
            ),
            icon_button(
                &crate::custom_id::ui::grant::user_page_custom_id(user, last),
                "⏩",
                ButtonStyle::Secondary,
                Some(page == last),
            ),
        ]));
    }
    Ok(container(Some(COLOR_BRAND as u32), children))
}

/// `/grant list`: the applications this guild has allowed to issue, newest first,
/// and the button that takes each permission back.
///
/// Paged with numbers rather than a cursor, as the contract screen is: first,
/// previous, next and last are what its arrows say, and a screen of five rows
/// needs all four. The count and the rows come from one call, because reading
/// every grant in the guild to print a number is the kind of read this list is
/// small enough to avoid.
async fn page(state: &AppState, guild_id: i64, page: i64) -> Result<Value, CommandError> {
    let authorized =
        vc_core::grant::authorized_in_guild(state.pool(), guild_id, page, MAX_APPLICATIONS as i64)
            .await?;

    let mut children = Vec::new();

    if authorized.total == 0 {
        children.push(text(crate::docs::discord::mentions(
            "発行を許可しているアプリケーションはありません。申請が来たときは、\
             アプリケーションが表示するコードを `/grant approve code:` に入れて承認します。",
            state.command_ids().await,
        )));
    } else {
        children.push(text(format!(
            "**発行を許可しているアプリケーション** ({}件)\n\
             取り消すと、そのアプリケーションはこのサーバーの発行枠から発行できなくなります。",
            authorized.total
        )));

        if authorized.applications.is_empty() {
            // A page a button led to that the list has since shrunk past: the
            // arrows below are the way back, and saying where the rows are is
            // better than saying there are none.
            children.push(text(
                "このページには何もありません。前のページに戻ってください。",
            ));
        }

        for application in &authorized.applications {
            // What the application may touch, by unit rather than by id: the
            // screen is where the narrowing stops being something only the API
            // knows, so a grant for one currency and one for all of them have to
            // read differently.
            let units = crate::resource::units(state.pool(), &application.resources).await?;

            children.push(text(format!(
                "**{}**\n`{}`\nこの申請は、{}を操作できます。",
                name_of(application.client_name.as_deref()),
                application.client_id,
                currencies(&units),
            )));
            children.push(action_row(vec![button(
                &revoke_custom_id(&application.client_id),
                "取り消す",
                ButtonStyle::Danger,
            )]));
        }

        if authorized.next.is_some() || authorized.page > 1 {
            children.push(pagination_row(&authorized));
        }
    }

    Ok(container(Some(COLOR_BRAND as u32), children))
}

/// Where the arrows move to, each disabled where there is nowhere to go.
fn pagination_row(authorized: &vc_core::grant::AuthorizedApplications) -> Value {
    let arrow = |at: u8, emoji: &str, target: Option<i64>, page: Page| -> Value {
        let id = match target {
            // A page that is not there is a button that says so, and one Discord
            // will not send: the id is a placeholder rather than this space's.
            None => format!("disabled-{at}"),
            Some(number) => page_custom_id(page, number),
        };

        icon_button(&id, emoji, ButtonStyle::Secondary, Some(target.is_none()))
    };

    action_row(vec![
        arrow(0, "⏪", authorized.first, Page::First),
        arrow(1, "⏮️", authorized.prev, Page::Previous),
        arrow(2, "⏭️", authorized.next, Page::Next),
        arrow(3, "⏩", authorized.last, Page::Last),
    ])
}

fn name_of(client_name: Option<&str>) -> String {
    client_name
        .filter(|name| !name.is_empty())
        .unwrap_or("(名前なし)")
        .to_string()
}

/// The currencies a grant covers, as the screens name them: 「すべての通貨」 when
/// the grant named none (or named the collection), and 「通貨 nyan だけ」 when it
/// named one. The two are what tell a narrowed grant from a wide one, which is
/// the whole reason the screen shows them — nothing else about the screen would
/// change, so the difference would otherwise be something only the API knows.
fn currencies(units: &[String]) -> String {
    if units.is_empty() {
        "すべての通貨".to_string()
    } else {
        format!("通貨 {} だけ", units.join("、"))
    }
}

/// A screen as the answer to what was typed or pressed: a new ephemeral message
/// when the command asked, and `UPDATE_MESSAGE` when the answer is a redraw of
/// the message the button came from.
fn answered(screen: Value, kind: i64) -> Value {
    json!({
        "type": kind,
        "data": ephemeral(vec![screen]),
    })
}

/// A refusal, as a screen of its own: a press that cannot be answered is worth a
/// sentence rather than a broken redraw.
fn error_screen(sentence: &str) -> Value {
    json!({
        "type": UPDATE_MESSAGE,
        "data": ephemeral(vec![container(
            Some(COLOR_ERROR as u32),
            vec![text(format!("エラー: {sentence}"))],
        )]),
    })
}

/// An ephemeral answer with nothing to act on, which is what a refusal to the
/// command itself is: a direct message has no administrator to be, and a member
/// without the bit is not one.
fn render_error(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": ephemeral(vec![container(
            None,
            vec![text(content)],
        )]),
    })
}
