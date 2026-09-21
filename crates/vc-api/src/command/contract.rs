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

use serde_json::{Map, Value, json};
use time::PrimitiveDateTime;

use super::claim::format_date_time;
use super::{
    CHANNEL_MESSAGE_WITH_SOURCE, COLOR_BRAND, COLOR_ERROR, CommandError, UPDATE_MESSAGE, get_user,
};
use crate::components::{ButtonStyle, action_row, button, container, ephemeral, text};
use crate::custom_id::ui::contract::{Action, custom_id};
use crate::error::ApiError;
use crate::state::AppState;
use vc_core::contract::{Contract, ContractError};

/// How many contracts one screen shows, for `/grant list`'s reason: five fits in
/// one message without scrolling it off the screen, and a sixth waits for the
/// screen the next answer redraws.
const MAX_CONTRACTS: usize = 5;

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
            page(state, payload).await?,
            CHANNEL_MESSAGE_WITH_SOURCE,
        )),
        _ => Err(CommandError::Unknown),
    }
}

/// A contract's button, pressed.
///
/// The answer is the screen drawn again as a change to the message that carried
/// the button: the contract it names is one of the ones listed, so the list the
/// press came from is the list to show next — and Discord sends nothing back but
/// the `custom_id`, which is why the contract id travelled in it.
pub async fn component(
    state: &AppState,
    custom_id: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    let (action, contract_id) =
        crate::custom_id::ui::contract::parse(&crate::custom_id::parse(custom_id))
            .map_err(|error| CommandError::Internal(ApiError::Internal(error.to_string())))?;

    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;
    let now = time::OffsetDateTime::now_utc();

    let decided = match action {
        Action::Approve => {
            vc_core::contract::approve(state.pool(), contract_id, account, now).await
        }
        Action::Refuse => vc_core::contract::refuse(state.pool(), contract_id, account, now).await,
        Action::Withdraw => {
            vc_core::contract::withdraw(state.pool(), contract_id, account, now).await
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

            Ok(answered(page(state, payload).await?, UPDATE_MESSAGE))
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

/// The screen: what this user was asked for, and what they can still answer.
///
/// A contract that is over is not shown — its money has gone home and there is
/// nothing left to press — and one that is not is shown with the state of the
/// caller's own part, which is the part they are deciding about.
async fn page(state: &AppState, payload: &Value) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;

    // The screen shows five, and the two things it says about the rest are the
    // page and the count: reading every contract this person is named in — with
    // every one's parties — to print a number was the one unbounded read on the
    // Discord side, and the number itself is one statement.
    let total = vc_core::contract::count_open_of_party(state.pool(), me)
        .await
        .map_err(contract_error)?;
    let open = vc_core::contract::open_of_party(state.pool(), me, MAX_CONTRACTS as i64)
        .await
        .map_err(contract_error)?;

    let mut children = Vec::new();

    if open.is_empty() {
        children.push(text(
            "あなたが対象になっている契約はありません。アプリケーションが契約を作ると、\
             ここに承認待ちとして並びます。",
        ));
    } else {
        children.push(text(format!(
            "**契約** ({total}件)\n承認すると、その分の通貨がロックされ、アプリケーションが操作できるようになります。",
        )));

        for contract in &open {
            children.push(text(describe(contract, me)));

            if let Some(row) = buttons(contract, me) {
                children.push(row);
            }
        }

        // What is left over is counted rather than the page's shortfall, so a
        // contract that ended between the two statements cannot make the screen
        // promise rows it will not show.
        let left = total - open.len() as i64;

        if left > 0 {
            children.push(text(format!("ほか{left}件。答えると一覧が進みます。")));
        }
    }

    Ok(container(Some(COLOR_BRAND as u32), children))
}

/// One contract, in four lines: who is asking, what the caller's part is, how far
/// the rest has come, and how long it lasts.
fn describe(contract: &Contract, me: i64) -> String {
    let name = contract
        .client_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or("（名前なし）");
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

    format!(
        "**{name}**（{unit}） — {}\nあなたの分: {amount}／{mine} ・ 承認 {approved}/{} ・ 残り {}\n{deadline}",
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
