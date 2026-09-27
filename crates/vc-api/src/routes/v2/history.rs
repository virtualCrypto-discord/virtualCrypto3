//! Two ledgers, read by token: what an account's wallet moved, and what a currency's pool
//! issued.
//!
//! The Elixir has no reader for either — its `/api/v1|2/…/transactions` are writers, its web has
//! no page for them and no command of its names a history — so both of these are additions, and
//! `docs/known-gaps.md` records them beside the Discord screens that show the same rows.
//! `vc_core::history` is the predicate the three share.
//!
//! `/users/@me/transactions` is the path the payment writer already lives on, which is why the
//! reader is a `GET` there rather than a route of its own: a caller that may pay may read what it
//! paid. It is the wallet's own movements, out of both ledgers: a payment names both sides — the
//! caller is one of them, and which one is the question — a lock names the caller on the sender
//! side and the contract's application on the other, a return names them the other way round, and
//! a charge credits its recipient without debiting the wallet that already locked the money.
//! An issuance is the pool paying the caller, which is money arriving like any
//! other, so it is here too, merged with the rest newest first. `event` and `ledger` say which of
//! those a row is.
//!
//! The merged order is not one column, so the cursor is not one number: `next` and `on_next` are
//! the last row's own place in it — see `vc_core::history::Place`, and the endpoint's `next` in
//! `docs/api.rs` for the spelling.
//!
//! `/currencies/{id}/issuances` is the guild's side of the one table, gated the way issuing is:
//! a guild token with `vc.issue`, reading the ledger of the one currency that token can spend
//! from. What may be seen follows what may be done, and this is where that was said before.

use axum::Json;
use axum::extract::{OriginalUri, Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::claims::format_timestamp;
use crate::error::ApiError;
use crate::routes::guild_token::GuildToken;
use crate::routes::limited::{Authorized, ReadTransactions};
use crate::routes::pagination::{self, QueryParams};
use crate::state::AppState;

/// `GET /api/v2/users/@me/transactions`: the caller's own ledger, newest first.
pub async fn transactions(
    State(state): State<AppState>,
    user: Authorized<ReadTransactions>,
    RawQuery(raw): RawQuery,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let params = QueryParams::parse(raw.as_deref().unwrap_or_default());

    let unit = params.one("unit");
    let related = params.one("related_discord_user_id");
    let counterparty = related.as_deref().map(related_id).transpose()?;

    // The cursor the endpoint writes is the merged order's own place, which is what
    // `vc_core::history` spells and reads — not the id this module's other lists use.
    let page = pagination::Page::asked_text(&params, vc_core::history::Place::parse)?
        .limited_to(pagination::PER_PAGE);

    let entries = vc_core::history::payments_after(
        state.pool(),
        user.account_id(),
        unit.as_deref(),
        counterparty,
        page.cursor,
        page.limit,
    )
    .await?;

    let next = page.next_text(&entries, |movement| movement.place().encode());
    let body = Value::Array(entries.iter().map(render_movement).collect());

    Ok(paged(
        body,
        next,
        page.limit,
        &[("unit", unit), ("related_discord_user_id", related)],
        &headers,
        uri.path(),
    ))
}

/// `GET /api/v2/currencies/{id}/issuances`: what that currency's pool has paid out, newest first.
pub async fn issuances(
    State(state): State<AppState>,
    guild: GuildToken,
    Path(id): Path<i64>,
    RawQuery(raw): RawQuery,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !guild.scopes.vc_issue {
        return Err(ApiError::InsufficientScope);
    }

    // The currency in the path has to be the guild's own. A guild token can spend one pool, and
    // reading a ledger is the other half of spending one — so a ledger belonging to a guild this
    // token is not for is answered the way a currency that is not there is, which is how every
    // other "not yours" in this service answers.
    if vc_core::history::guild_currency(state.pool(), guild.guild_id).await? != Some(id) {
        return Err(ApiError::NotFound);
    }
    crate::resource::ensure(&guild.resources, id)?;

    let params = QueryParams::parse(raw.as_deref().unwrap_or_default());
    let related = params.one("related_discord_user_id");
    let receiver = related.as_deref().map(related_id).transpose()?;

    let page = pagination::Page::asked(&params)?.limited_to(pagination::PER_PAGE);

    let entries =
        vc_core::history::issuances_after(state.pool(), id, receiver, page.cursor, page.limit)
            .await?;

    let next = page
        .next_cursor(&entries, |issuance| issuance.id)
        .map(|id| id.to_string());
    let body = Value::Array(entries.iter().map(render_issuance).collect());

    Ok(paged(
        body,
        next,
        page.limit,
        &[("related_discord_user_id", related)],
        &headers,
        uri.path(),
    ))
}

/// One movement of the caller's ledger, whichever ledger it came from.
///
/// `ledger` is the table the row is a row of, and the two count their ids separately — so it is
/// what makes `id` mean something in a list of both. `event` is what kind of movement it is
/// within that ledger: `lock` when a party locked money into a contract (the caller on the sender
/// side, `null` on the receiver), `return` when a contract gave money back (the other way round),
/// `charge` when this wallet received a contract payment, `issue` when the pool paid
/// the caller, and `null` for a payment between people.
fn render_movement(movement: &vc_core::history::Movement) -> Value {
    match movement {
        vc_core::history::Movement::Payment(payment) => json!({
            "id": payment.id.to_string(),
            "ledger": "payment",
            "amount": payment.amount.to_string(),
            "balance_after": payment.balance_after.map(|amount| amount.to_string()),
            "unit": payment.unit,
            "sender_discord_id": payment.sender_discord_id.map(|id| id.to_string()),
            "receiver_discord_id": payment.receiver_discord_id.map(|id| id.to_string()),
            "contract_client_name": payment.contract_client_name,
            "event": payment.event,
            "time": format_timestamp(payment.time),
        }),
        // No sender at all: the pool paid, and the pool has no Discord id to name.
        vc_core::history::Movement::Issuance(issuance) => json!({
            "id": issuance.id.to_string(),
            "ledger": "issuance",
            "amount": issuance.amount.to_string(),
            "balance_after": issuance.balance_after.map(|amount| amount.to_string()),
            "unit": issuance.unit,
            "sender_discord_id": Value::Null,
            "receiver_discord_id": issuance.receiver_discord_id.map(|id| id.to_string()),
            "contract_client_name": Value::Null,
            "event": "issue",
            "time": format_timestamp(issuance.time),
        }),
    }
}

/// One issuance. There is no sender: the pool paid, and the currency in the path says which.
fn render_issuance(issuance: &vc_core::history::Issuance) -> Value {
    json!({
        "id": issuance.id.to_string(),
        "amount": issuance.amount.to_string(),
        "pool_balance_after": issuance.pool_balance_after.map(|amount| amount.to_string()),
        "receiver_discord_id": issuance.receiver_discord_id.map(|id| id.to_string()),
        "time": format_timestamp(issuance.time),
    })
}

/// A related-user id, which is the filter the claim list has under the same name and refuses the
/// same way: a Discord id is digits, and a caller that sent something else named a filter that
/// cannot be read rather than one that matches nothing.
fn related_id(value: &str) -> Result<i64, ApiError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::InvalidRequest("invalid_related_user"));
    }

    pagination::parse_number(value)
        .filter(|id| *id > 0)
        .ok_or(ApiError::InvalidRequest("invalid_related_user"))
}

/// A list's answer: the body, and a `link` header when there is a next page.
///
/// The filters travel in the link, unlike the contract family's three lists — those are filtered
/// by who is asking and by nothing else, and a page 2 that dropped one of these would be a
/// different list. The cursor travels as the list spelled it, because it is the list's own: an id
/// for the issuance ledger, and the merged ledger's `time:ledger:id` for the payments one.
/// Encode query values: currencies imported from v2 may have units containing
/// reserved characters even though new currency creation now refuses them.
fn paged(
    body: Value,
    next: Option<String>,
    limit: Option<i64>,
    filters: &[(&str, Option<String>)],
    headers: &HeaderMap,
    path: &str,
) -> Response {
    let mut response = Json(body).into_response();

    if let Some(next) = next {
        let mut url = reqwest::Url::parse("http://placeholder/").expect("a base url");
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", &limit.unwrap_or_default().to_string());
            query.append_pair("next", &next);
            for (name, value) in filters {
                if let Some(value) = value {
                    query.append_pair(name, value);
                }
            }
        }

        if let Some(value) = pagination::link(headers, path, url.query().unwrap_or_default()) {
            response.headers_mut().insert("link", value);
        }
    }

    response
}
