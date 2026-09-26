//! `/contract`: the contracts a user is named in, and the buttons that answer
//! them.
//!
//! Not from the Elixir — its contract page is a mockup whose buttons carry no
//! `phx-click`, and it has no Discord surface for contracts at all — so what this
//! mirrors is `/grant`'s shape: one ephemeral screen, drawn from what a read
//! answers. What differs is whose screen it is: a contract is between an
//! application and the *user*, so there is no administrator bit to ask for and no
//! guild to be in — the screen is the caller's own, which is why this command
//! runs in a DM as well as in a guild.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use time::PrimitiveDateTime;

use super::claim::format_date_time;
use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, CommandError, UPDATE_MESSAGE, get_user,
};
use crate::components::{ButtonStyle, action_row, button, container, ephemeral, text};
use crate::custom_id::ui::contract::{Action, Page, Pressed, custom_id, page_custom_id};
use crate::error::ApiError;
use crate::state::AppState;
use vc_core::contract::{Contract, ContractError};

/// How many contracts one screen shows, for the grant list's reason: five fits in
/// one message without scrolling it off the screen, and a sixth waits for the
/// screen the next answer redraws.
const MAX_CONTRACTS: usize = 5;

/// Where a screen starts, and where a decision draws next: page numbers count from
/// one, as the claim list's do.
const FIRST_PAGE: i64 = 1;

/// Decisions acknowledge before acquiring any database locks. Pagination keeps
/// its inline response because it does not mutate a contract.
pub async fn respond(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Response, CommandError> {
    let pressed = crate::custom_id::ui::contract::parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;
    if matches!(pressed, Pressed::Paged(_, _)) {
        return Ok(Json(component(state, custom_id, payload).await?).into_response());
    }
    let reply = super::response::Acknowledged::update(state, payload).await?;
    let state = state.clone();
    let custom_id = custom_id.to_owned();
    let payload = payload.clone();
    tokio::spawn(async move {
        let response = component(&state, &custom_id, &payload)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(?error, "Discord contract operation failed");
                error_screen(
                    "契約の処理結果を確認できませんでした。契約一覧と履歴を確認してください。",
                )
            });
        reply.edit(response["data"].clone()).await;
    });
    Ok(StatusCode::ACCEPTED.into_response())
}

/// `Command.handle/4` for `contract`: the subcommand picks what to draw.
pub async fn handle(
    state: &AppState,
    options: &Map<String, Value>,
    payload: &Value,
) -> Result<Value, CommandError> {
    let subcommand = options
        .get("subcommand")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::missing("contract has no subcommand"))?;

    match subcommand {
        "list" => Ok(answered(
            page(state, payload, FIRST_PAGE).await?,
            CHANNEL_MESSAGE_WITH_SOURCE,
        )),
        _ => Err(CommandError::Unknown),
    }
}

/// A contract's button, pressed: an answer about one contract, or a move from one
/// page of the list to another.
///
/// The answer is the screen drawn again as a change to the message that carried
/// the button, and Discord sends nothing back but the `custom_id` — which is why
/// the contract id, or the page, travelled in it.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let pressed = crate::custom_id::ui::contract::parse(&crate::custom_id::parse(custom_id))
        .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    // A page button is the whole of its own answer: nothing is decided, and the
    // screen it draws is the page it named.
    let (action, contract_id) = match pressed {
        Pressed::Paged(_, number) => {
            return Ok(answered(
                page(state, payload, number).await?,
                UPDATE_MESSAGE,
            ));
        }
        Pressed::Decided(action, contract_id) => (action, contract_id),
    };

    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;
    let clock = time::OffsetDateTime::now_utc;

    let decided = match action {
        Action::Approve => {
            vc_core::contract::approve(state.pool(), contract_id, account, clock).await
        }
        Action::Refuse => {
            vc_core::contract::refuse(state.pool(), contract_id, account, clock()).await
        }
        Action::Withdraw => {
            vc_core::contract::withdraw(state.pool(), contract_id, account, clock).await
        }
    };

    match decided {
        Ok(decided) => {
            // Only a decision is told: pressing 承認する twice is not a second one.
            if decided {
                let found = vc_core::contract::find(state.pool(), contract_id)
                    .await
                    .map_err(contract_error)?;

                if let Some(contract) = found {
                    state
                        .notifier()
                        .notify_contract_decided(contract.application_id, contract_id);
                }
            }

            // The first page, because a decision's button says which contract it is
            // about and not which page it was on — the claim list's answer buttons
            // carry no position either, and a decision that ends a contract moves
            // the rows under it anyway.
            Ok(answered(
                page(state, payload, FIRST_PAGE).await?,
                UPDATE_MESSAGE,
            ))
        }
        Err(ContractError::NotFound) => Ok(error_screen("その契約はあなたを対象にしていません。")),
        Err(ContractError::NotEnoughAmount) => Ok(error_screen("お金が足りません。")),
        Err(ContractError::InvalidStatus) => Ok(error_screen(
            "その契約には今この操作ができません。期限のある契約は、期間中は取り消せません。",
        )),
        Err(ContractError::Expired) => Ok(error_screen("その契約の期限は切れています。")),
        Err(error) => Err(CommandError::Internal(ApiError::Internal(format!(
            "the contract decision failed: {error:?}"
        )))),
    }
}

/// The screen: what this user was asked for, what they can still answer, and the
/// arrows to the rest of it.
///
/// A contract that is over is not shown — its money has gone home and there is
/// nothing left to press — and one that is not is shown with the state of the
/// caller's own part, which is the part they are deciding about.
///
/// **The two mechanisms are both here, and each where it belongs**: the API pages
/// this family with a cursor (`next`/`on_next`, and the statement's `link` header),
/// and this screen pages it with numbers, because first, previous, next and last
/// are page numbers and a cursor cannot say "back". `vc_core::contract` has one
/// read for each — `of_party` for the cursor, `open_of_party` for the page.
async fn page(state: &AppState, payload: &Value, page: i64) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    // Five rows, the count behind them, and the four pages around this one: one
    // call, because reading every contract this person is named in — with every
    // one's parties — to print a number was the one unbounded read on the Discord
    // side.
    let open = vc_core::contract::open_of_party(state.pool(), me, page, MAX_CONTRACTS as i64)
        .await
        .map_err(contract_error)?;

    let mut children = Vec::new();

    if open.total == 0 {
        children.push(text(
            "あなたが対象になっている契約はありません。アプリケーションが契約を作ると、\
             ここに承認待ちとして並びます。",
        ));
    } else {
        children.push(text(format!(
            "**契約** ({}件)\n承認すると、その分の通貨がロックされ、アプリケーションが操作できるようになります。",
            open.total
        )));

        if open.contracts.is_empty() {
            // A page a button led to that the list has since shrunk past: the
            // arrows below are the way back, and saying where the contracts are is
            // better than saying there are none.
            children.push(text(
                "このページには何もありません。前のページに戻ってください。",
            ));
        }

        for contract in &open.contracts {
            children.push(text(describe(contract, me)));

            if let Some(row) = buttons(contract, me) {
                children.push(row);
            }
        }

        // The claim list's pagination row without its reload button: that screen
        // has one because its list carries filters, and this one has none. It is
        // drawn whenever there is another page — or a page to come back from, which
        // is what a contract ending under a button leaves behind.
        if open.next.is_some() || open.page > 1 {
            children.push(pagination_row(&open));
        }
    }

    Ok(container(Some(COLOR_BRAND as u32), children))
}

/// Where the arrows move to, each disabled where there is nowhere to go.
fn pagination_row(open: &vc_core::contract::OpenContracts) -> Value {
    let arrow = |at: u8, emoji: &str, target: Option<i64>, page: Page| -> Value {
        let custom_id = match target {
            // A page that is not there is a button that says so, and one Discord
            // will not send: the id is a placeholder rather than this space's.
            None => format!("disabled-{at}"),
            Some(number) => page_custom_id(page, number),
        };

        crate::components::icon_button(
            &custom_id,
            emoji,
            crate::components::ButtonStyle::Secondary,
            Some(target.is_none()),
        )
    };

    crate::components::action_row(vec![
        arrow(0, "⏪", open.first, Page::First),
        arrow(1, "⏮️", open.prev, Page::Previous),
        arrow(2, "⏭️", open.next, Page::Next),
        arrow(3, "⏩", open.last, Page::Last),
    ])
}

/// One contract: who is asking, what the caller's part is, how far
/// the rest has come, where money may go, and how long it lasts.
fn describe(contract: &Contract, me: i64) -> String {
    let application = super::application_identity(
        contract.bot_discord_id,
        &contract.client_id,
        contract.client_name.as_deref(),
    );
    let unit = contract.unit.as_deref().unwrap_or("（単位なし）");

    let mine = contract.parties.iter().find(|party| party.discord_id == me);
    let (amount, mine) = match mine {
        Some(party) => (party.amount, party_status(&party.status).to_string()),
        None => (0, "対象外".to_string()),
    };
    let approved = contract
        .parties
        .iter()
        .filter(|party| party.status == "approved")
        .count();
    let deadline = match contract.expires_at {
        Some(at) => format!("期限: {}", format_date_time(at)),
        None => "期限: なし（いつでも取り消せます）".to_string(),
    };
    let receiver = match contract.receiver_discord_id {
        Some(id) => format!("送金先: <@{id}> に限定"),
        None => "送金先: 制限なし".to_string(),
    };

    format!(
        "{application}\n（{unit}） — {}\nあなたの分: {amount}／{mine} ・ 承認 {approved}/{} ・ 残り {}\n{receiver}\n{deadline}",
        contract_status(&contract.status),
        contract.parties.len(),
        contract.remaining,
    )
}

/// The buttons a contract can offer the caller, if it can offer any.
///
/// Nothing is offered on a contract whose part is already decided and cannot be
/// taken back — one more press would only be refused, and a screen that offers
/// what it will refuse is a screen that lies.
fn buttons(contract: &Contract, me: i64) -> Option<Value> {
    if contract.status != "pending" && contract.status != "active" {
        return None;
    }
    let mine = contract
        .parties
        .iter()
        .find(|party| party.discord_id == me)?;

    match mine.status.as_str() {
        "pending" => Some(action_row(vec![
            button(
                &custom_id(Action::Approve, contract.id),
                "承認する",
                ButtonStyle::Success,
            ),
            button(
                &custom_id(Action::Refuse, contract.id),
                "拒否する",
                ButtonStyle::Danger,
            ),
        ])),
        "approved" if withdrawable(contract) => Some(action_row(vec![button(
            &custom_id(Action::Withdraw, contract.id),
            "取り消す",
            ButtonStyle::Secondary,
        )])),
        _ => None,
    }
}

/// Whether a party may take their part back: a permanent contract any time, a
/// temporary one once the period it agreed to has run out.
fn withdrawable(contract: &Contract) -> bool {
    let now = time::OffsetDateTime::now_utc();
    let now = PrimitiveDateTime::new(now.date(), now.time());

    contract.expires_at.is_none_or(|expires| expires <= now)
}

fn contract_status(status: &str) -> &'static str {
    match status {
        "active" => "全員承認済み",
        "canceled" => "終了",
        "expired" => "期限切れ",
        _ => "承認待ち",
    }
}

fn party_status(status: &str) -> &'static str {
    match status {
        "approved" => "承認済み",
        "refused" => "拒否",
        "withdrawn" => "取り消し済み",
        _ => "未回答",
    }
}

/// A screen as the answer to what was pressed or typed: `UPDATE_MESSAGE` when it
/// is a redraw of the message the button came from, and a new ephemeral message
/// when the command asked.
fn answered(screen: Value, kind: i64) -> Value {
    json!({
        "type": kind,
        "data": ephemeral(vec![screen]),
    })
}

/// A refusal, as a screen of its own: a button that cannot be answered is worth a
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

fn contract_error(error: ContractError) -> CommandError {
    match error {
        ContractError::Database(error) => CommandError::from(error),
        other => CommandError::Internal(ApiError::Internal(format!("{other:?}"))),
    }
}
