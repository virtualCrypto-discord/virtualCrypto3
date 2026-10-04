//! Common grant approval and separate personal/server grant management.
//! Every button rechecks the interaction actor and the request's live state.

use serde_json::{Map, Value, json};

use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, COLOR_OK, CommandError, UPDATE_MESSAGE,
    as_int, as_permissions, get_user, is_administrator, value_text,
};
use crate::components::{
    ButtonStyle, action_row, button, container, ephemeral, icon_button, separator, text,
};
use crate::custom_id::ui::grant::{self as ids, Page, Pressed, page_custom_id};
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
            review_screen(
                state,
                ReviewDisplay::Pending(review),
                1,
                CHANNEL_MESSAGE_WITH_SOURCE,
            )
            .await
        }
        "user" => Ok(answered(
            personal_page(state, user, FIRST_PAGE).await?,
            CHANNEL_MESSAGE_WITH_SOURCE,
        )),
        // Keep already-issued /grant list interactions as a server-list alias.
        "server" | "list" => {
            if payload.get("guild_id").and_then(as_int).is_none() {
                return Ok(render_error(message!("command.grant.handle.001")));
            }
            let Some(guild) = authorized_guild(payload) else {
                return Ok(render_error(message!("command.grant.handle.002")));
            };
            Ok(answered(
                page(state, guild, FIRST_PAGE).await?,
                CHANNEL_MESSAGE_WITH_SOURCE,
            ))
        }
        _ => Err(CommandError::Unknown),
    }
}

const NOT_FOUND: &str = message!("command.grant.handle.003");

fn actor(payload: &Value) -> Result<i64, CommandError> {
    get_user(payload).ok_or_else(|| CommandError::missing("grant has no user"))
}

pub(super) fn authorized_guild(payload: &Value) -> Option<i64> {
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
        Pressed::ReviewPage(request, page) => {
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
            review_screen(state, ReviewDisplay::Pending(review), page, UPDATE_MESSAGE).await
        }
        Pressed::Details(grant, page) => {
            let Some(review) =
                vc_core::grant::grant_details(state.pool(), grant, user, authorized_guild(payload))
                    .await?
            else {
                return Ok(error_screen(message!("command.grant.component.001")));
            };
            review_screen(state, ReviewDisplay::Granted(&review), page, UPDATE_MESSAGE).await
        }
        Pressed::RevokeOne(grant) => {
            let guild = authorized_guild(payload);
            let Some(review) =
                vc_core::grant::grant_details(state.pool(), grant, user, guild).await?
            else {
                return Ok(error_screen(message!("command.grant.component.002")));
            };
            if vc_core::grant::revoke_one(state.pool(), grant, user, guild).await? {
                state.notifier().notify_delegation_decided(
                    review.application_id,
                    review.target,
                    grant,
                    &[],
                );
            }
            Ok(answered(
                match review.target {
                    Target::User(id) => personal_page(state, id, FIRST_PAGE).await?,
                    Target::Guild(id) => page(state, id, FIRST_PAGE).await?,
                },
                UPDATE_MESSAGE,
            ))
        }
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
            let Some(grant_id) = vc_core::grant::approve_review(
                state.pool(),
                review,
                time::OffsetDateTime::now_utc(),
            )
            .await?
            else {
                return Ok(error_screen(NOT_FOUND));
            };
            state.notifier().notify_delegation_decided(
                review.application_id,
                review.target,
                grant_id,
                &review.scopes,
            );
            let message = match review.target {
                Target::Guild(_) => format!(
                    message!("command.grant.component.003"),
                    currency_label(state, &review.resources).await?.0
                ),
                Target::User(_) => format!(
                    message!("command.grant.component.004"),
                    permissions(&review.scopes),
                    currency_label(state, &review.resources).await?.0
                ),
            };
            Ok(answered(
                container(Some(COLOR_OK as u32), vec![text(message)]),
                UPDATE_MESSAGE,
            ))
        }
        Pressed::UserPaged(owner, number) => {
            if owner != user {
                return Ok(error_screen(message!("command.grant.component.005")));
            }
            Ok(answered(
                personal_page(state, user, number).await?,
                UPDATE_MESSAGE,
            ))
        }
        Pressed::UserRevoked(_, _) => Ok(error_screen(message!("command.grant.component.006"))),
        Pressed::Paged(_, number) => {
            let Some(guild) = authorized_guild(payload) else {
                return Ok(error_screen(message!("command.grant.component.007")));
            };
            Ok(answered(page(state, guild, number).await?, UPDATE_MESSAGE))
        }
        Pressed::Revoked(_) => Ok(error_screen(message!("command.grant.component.008"))),
    }
}

fn permissions(scopes: &[String]) -> String {
    if scopes.is_empty() {
        return message!("command.grant.permissions.001").to_owned();
    }
    scopes
        .iter()
        .map(|scope| {
            let description = if scope == "vc.issue" {
                message!("command.grant.permissions.002")
            } else {
                vc_core::delegation::Scope::parse(scope)
                    .map(|scope| scope.description())
                    .unwrap_or(message!("command.grant.permissions.003"))
            };
            format!("- {description}")
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
        message!("command.grant.personal_page.001"),
        last = last,
        page = page,
        total = total
    ))];
    if apps.is_empty() {
        children.push(text(message!("command.grant.personal_page.002")));
    }
    for app in apps {
        let (label, needs_details) = currency_label(state, &app.resources).await?;
        children.push(separator());
        children.push(text(format!(
            message!("command.grant.personal_page.003"),
            super::application_identity(
                app.bot_discord_id,
                &app.client_id,
                app.client_name.as_deref()
            ),
            permissions(&app.scopes),
            label
        )));
        let mut actions = vec![button(
            &ids::revoke_one_custom_id(app.grant_id),
            message!("command.grant.personal_page.004"),
            ButtonStyle::Danger,
        )];
        if needs_details {
            actions.push(button(
                &ids::details_custom_id(app.grant_id, 1),
                message!("command.grant.personal_page.005"),
                ButtonStyle::Secondary,
            ));
        }
        children.push(action_row(actions));
    }
    children.push(separator());
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
            message!("command.grant.page.001"),
            state.command_ids().await,
        )));
    } else {
        children.push(text(format!(
            message!("command.grant.page.002"),
            authorized.total
        )));

        if authorized.applications.is_empty() {
            // A page a button led to that the list has since shrunk past: the
            // arrows below are the way back, and saying where the rows are is
            // better than saying there are none.
            children.push(text(message!("command.grant.page.003")));
        }

        for application in &authorized.applications {
            let (label, needs_details) = currency_label(state, &application.resources).await?;
            // What the application may touch, by unit rather than by id: the
            // screen is where the narrowing stops being something only the API
            // knows, so a grant for one currency and one for all of them have to
            // read differently.

            children.push(separator());
            children.push(text(format!(
                message!("command.grant.page.004"),
                super::application_identity(
                    application.bot_discord_id,
                    &application.client_id,
                    application.client_name.as_deref()
                ),
                label,
            )));
            let mut actions = vec![button(
                &ids::revoke_one_custom_id(application.grant_id),
                message!("command.grant.page.005"),
                ButtonStyle::Danger,
            )];
            if needs_details {
                actions.push(button(
                    &ids::details_custom_id(application.grant_id, 1),
                    message!("command.grant.page.006"),
                    ButtonStyle::Secondary,
                ));
            }
            children.push(action_row(actions));
        }
    }
    children.push(separator());
    children.push(pagination_row(&authorized));

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

async fn currency_label(
    state: &AppState,
    resources: &[i64],
) -> Result<(String, bool), CommandError> {
    if resources.is_empty() {
        return Ok((
            message!("command.grant.currency_label.001").to_owned(),
            false,
        ));
    }
    let units = crate::resource::units(state.pool(), resources).await?;
    let label = format!(
        message!("command.grant.currency_label.002"),
        units.join("、")
    );
    if label.chars().count() <= 300 {
        Ok((label, false))
    } else {
        Ok((
            format!(
                message!("command.grant.currency_label.003"),
                resources.len()
            ),
            true,
        ))
    }
}

/// Keep the complete consent visible without exceeding Discord's message limits.
/// Currency entries are at most 255 characters in the database; a page uses at
/// most 1200 characters, leaving room for all operation descriptions and identity.
enum ReviewDisplay<'a> {
    Pending(&'a vc_core::grant::Review),
    Granted(&'a vc_core::grant::GrantDetails),
}

async fn review_screen(
    state: &AppState,
    display: ReviewDisplay<'_>,
    requested: i64,
    kind: i64,
) -> Result<Value, CommandError> {
    let (key, target, client_name, client_id, bot_discord_id, scopes, resources, pending) =
        match display {
            ReviewDisplay::Pending(r) => (
                r.request_id,
                r.target,
                &r.client_name,
                &r.client_id,
                r.bot_discord_id,
                &r.scopes,
                &r.resources,
                true,
            ),
            ReviewDisplay::Granted(g) => (
                g.grant_id,
                g.target,
                &g.client_name,
                &g.client_id,
                g.bot_discord_id,
                &g.scopes,
                &g.resources,
                false,
            ),
        };
    let units = crate::resource::units(state.pool(), resources).await?;
    let mut pages = vec![String::new()];
    if units.is_empty() {
        pages[0] = message!("command.grant.review_screen.001").to_owned();
    }
    for unit in &units {
        let line = format!("- {unit}\n");
        if pages.last().unwrap().chars().count() + line.chars().count() > 1200 {
            pages.push(String::new());
        }
        pages.last_mut().unwrap().push_str(&line);
    }
    let last = pages.len() as i64;
    let page = requested.clamp(1, last);
    let target_label = match target {
        Target::User(id) => format!(message!("command.grant.review_screen.002"), id = id),
        Target::Guild(id) => format!(message!("command.grant.review_screen.003"), id = id),
    };
    let heading = if pending {
        message!("command.grant.review_screen.004")
    } else {
        message!("command.grant.review_screen.005")
    };
    let mut children = vec![text(format!(
        message!("command.grant.review_screen.006"),
        super::application_identity(bot_discord_id, client_id, client_name.as_deref()),
        permissions(scopes),
        if units.is_empty() {
            message!("command.grant.review_screen.007").to_owned()
        } else {
            format!(message!("command.grant.review_screen.008"), units.len())
        },
        pages[(page - 1) as usize],
        heading = heading,
        last = last,
        page = page,
        target_label = target_label
    ))];
    if pending {
        children.push(text(message!("command.grant.review_screen.009")));
    }
    let id = |p| {
        if pending {
            ids::review_page_custom_id(key, p)
        } else {
            ids::details_custom_id(key, p)
        }
    };
    children.push(action_row(vec![
        icon_button(&id(1), "⏪", ButtonStyle::Secondary, Some(page == 1)),
        icon_button(
            &id((page - 1).max(1)),
            "⏮️",
            ButtonStyle::Secondary,
            Some(page == 1),
        ),
        icon_button(
            &id((page + 1).min(last)),
            "⏭️",
            ButtonStyle::Secondary,
            Some(page == last),
        ),
        icon_button(&id(last), "⏩", ButtonStyle::Secondary, Some(page == last)),
    ]));
    // Keep the action in a consistent position across currency pages.
    children.insert(
        1,
        action_row(vec![if pending {
            button(
                &ids::confirm_custom_id(key),
                message!("command.grant.review_screen.010"),
                ButtonStyle::Success,
            )
        } else {
            button(
                &ids::revoke_one_custom_id(key),
                message!("command.grant.review_screen.011"),
                ButtonStyle::Danger,
            )
        }]),
    );
    if !pending {
        let back = match target {
            Target::User(user) => ids::user_page_custom_id(user, 1),
            Target::Guild(_) => ids::page_custom_id(Page::First, 1),
        };
        children.push(action_row(vec![button(
            &back,
            message!("command.grant.review_screen.012"),
            ButtonStyle::Secondary,
        )]));
    }
    Ok(answered(
        container(Some(COLOR_BRAND as u32), children),
        kind,
    ))
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
            vec![text(format!(message!("command.grant.error_screen.001"), sentence = sentence))],
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
            Some(COLOR_ERROR as u32),
            vec![text(content)],
        )]),
    })
}
