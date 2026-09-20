//! `/api/v2/contracts`: an application operating a user's currency, on the
//! strength of the parties' approvals.
//!
//! **An addition, with no golden to reproduce.** The Elixir's contract page is a
//! mockup with no endpoint behind it (`docs/web-ui.md`), so the shapes here are
//! this family's own: ids and amounts as strings, the claim endpoints' timestamp
//! format, and refusals that say which of the two things is wrong — a body or a
//! name in `error_description`, a state in `error_info`, which is the line the
//! claim endpoints already draw. `docs/contracts.md` records the design.
//!
//! Two callers, and each endpoint is one of them: the application that wrote a
//! contract, with an `app` token carrying `vc.contract`, and the users it names,
//! with their own `user` tokens. The scope is not what lets the application
//! spend — the parties' approvals are.

use axum::Json;
use axum::extract::{OriginalUri, Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use time::OffsetDateTime;

use vc_auth::AuthUser;

use crate::error::ApiError;
use crate::routes::idempotency::{self, Idempotency};
use crate::routes::limited::Limited;
use crate::routes::pagination::{self, QueryParams};
use crate::routes::v2::claims::format_timestamp;
use crate::state::AppState;
use vc_core::contract::{self, Contract, ContractError, NewParty};

/// How many payments a statement page holds when the caller does not say. A
/// statement is paged whether or not anyone asked, because unlike the lists
/// beside it — whose absent `limit` still means every row — its rows are written
/// by every use an application bills for.
const PAYMENTS_PER_PAGE: i64 = 50;

/// `POST /api/v2/contracts`: the application asking.
///
/// Nothing is locked and nothing is agreed here: this is the question the named
/// users answer.
pub async fn create(
    State(state): State<AppState>,
    user: Limited,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let application = application(&state, &user).await?;
    let asked = asked(&body)?;

    let id = contract::create(
        state.pool(),
        application,
        &asked.unit,
        &asked.parties,
        asked.receiver_discord_id,
        asked.expires_in,
        OffsetDateTime::now_utc(),
    )
    .await
    .map_err(contract_error)?;

    let created = find(&state, id).await?;

    Ok((StatusCode::CREATED, Json(render(&created))).into_response())
}

/// `GET /api/v2/contracts`: the ones this application wrote, newest first.
///
/// `limit` is what makes it a page. Without it the answer is every contract, the
/// way it was before it could be paged; with it, a page that came back full
/// carries the `link` header that continues from its last id.
pub async fn index(
    State(state): State<AppState>,
    user: Limited,
    RawQuery(raw): RawQuery,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let application = application(&state, &user).await?;

    let params = QueryParams::parse(raw.as_deref().unwrap_or_default());
    let page = pagination::Page::asked(&params)?;

    let contracts = contract::of_application(state.pool(), application, page.cursor, page.limit)
        .await
        .map_err(contract_error)?;

    let next = page.next_cursor(&contracts, |contract| contract.id);

    Ok(paged(
        rendered(&contracts),
        next,
        page.limit,
        &headers,
        uri.path(),
    ))
}

/// `GET /api/v2/contracts/{id}`: the application that wrote it, or one of the
/// users it names.
///
/// A caller who is neither is answered as a contract that is not there — the
/// same nothing a wrong id is, so this cannot be used to ask which ids exist.
pub async fn show(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let found = find(&state, id).await?;

    if !visible(&state, &user, &found).await? {
        return Err(ApiError::NotFound);
    }

    Ok(Json(render(&found)))
}

/// `GET /api/v2/contracts/{id}/balances`: what the parties hold of the
/// contract's currency.
///
/// The balance read the parties' approvals carry: an application cannot decide
/// what to pay out without knowing what the people it pays hold.
pub async fn balances(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let application = application(&state, &user).await?;

    let balances = contract::party_balances(state.pool(), id, application)
        .await
        .map_err(contract_error)?;

    Ok(Json(Value::Array(
        balances
            .iter()
            .map(|balance| {
                json!({
                    "discord_id": balance.discord_id.to_string(),
                    "amount": balance.amount.to_string(),
                })
            })
            .collect(),
    )))
}

/// `POST /api/v2/contracts/{id}/approval`: a party locking their amount.
///
/// Approving twice is not a second decision, and it is answered the same way:
/// the money is already locked, and there is nothing to do but say what the
/// contract looks like now.
///
/// A decision is also what the application is told about, so approving twice
/// tells it once.
pub async fn approve(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let account = account(&user)?;

    let decided = contract::approve(state.pool(), id, account, OffsetDateTime::now_utc())
        .await
        .map_err(contract_error)?;

    let contract = find(&state, id).await?;

    if decided {
        state
            .notifier()
            .notify_contract_decided(contract.application_id, id);
    }

    Ok(Json(render(&contract)))
}

/// `POST /api/v2/contracts/{id}/refusal`: a party saying no before they have
/// said yes.
///
/// The contract cannot become what it was written as, so it is over and the
/// parties who had already locked their amounts take back what is left.
pub async fn refuse(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let account = account(&user)?;

    let decided = contract::refuse(state.pool(), id, account, OffsetDateTime::now_utc())
        .await
        .map_err(contract_error)?;

    let contract = find(&state, id).await?;

    if decided {
        state
            .notifier()
            .notify_contract_decided(contract.application_id, id);
    }

    Ok(Json(render(&contract)))
}

/// `DELETE /api/v2/contracts/{id}/approval`: a party taking the delegation back.
///
/// Allowed while a permanent contract stands and after a temporary one has run
/// out; a temporary contract still running is the one case it is refused,
/// because the period is what the party agreed to.
pub async fn withdraw(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let account = account(&user)?;

    let decided = contract::withdraw(state.pool(), id, account, OffsetDateTime::now_utc())
        .await
        .map_err(contract_error)?;

    let contract = find(&state, id).await?;

    if decided {
        state
            .notifier()
            .notify_contract_decided(contract.application_id, id);
    }

    Ok(Json(render(&contract)))
}

/// `POST /api/v2/contracts/{id}/payments`: the application spending what the
/// parties locked.
///
/// The one write in this family that an `Idempotency-Key` matters for, and the
/// reason the family has one at all: a charge that is retried because the answer
/// was lost is a subscriber billed twice, and a billing API that cannot be
/// retried safely is one nobody can reconcile. The key is the application's, so
/// two applications may use the same one; a refusal is stored as well, because
/// "the quota is gone" is an answer a retry must get rather than a second
/// attempt at.
pub async fn pay(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let account = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("the token's subject is not an account id".into()))?;

    let claimed =
        match idempotency::claim(&state, &headers, account, user.scopes.vc_contract).await? {
            Idempotency::None => None,
            Idempotency::Claimed(key) => Some(key),
            Idempotency::Answered(response) => return Ok(response),
        };

    let (status, body) = paid(&state, &user, id, &body).await?;

    if let Some(key) = claimed {
        idempotency::register(&state, &key, account, status, &body).await?;

        Ok(idempotency::with_idempotency(status, body, "OK"))
    } else {
        Ok(idempotency::with_idempotency(status, body, "Not Requested"))
    }
}

/// The charge itself, as the pair an answer is: what the idempotency layer
/// stores and what the caller is given.
///
/// A domain refusal is `Ok` here rather than an `Err`, because a refusal *is* an
/// answer — the one a replay has to return instead of charging again. What stays
/// an error is the database failing, which is not an answer and must not be
/// handed back later as though it were.
async fn paid(
    state: &AppState,
    user: &AuthUser,
    id: i64,
    body: &Value,
) -> Result<(StatusCode, Value), ApiError> {
    let application = application(state, user).await?;
    let payment = payment(body)?;

    let payed = match contract::pay(
        state.pool(),
        id,
        application,
        payment.receiver_discord_id,
        payment.party_discord_id,
        payment.amount,
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(payed) => payed,
        Err(ContractError::Database(error)) => {
            return Err(ApiError::Core(vc_core::Error::Database(error)));
        }
        Err(refusal) => return Ok(contract_error(refusal).parts()),
    };

    let unit = find(state, id).await?.unit;

    Ok((
        StatusCode::CREATED,
        json!({
            "amount": payed.amount.to_string(),
            "remaining": payed.remaining.to_string(),
            "unit": unit,
        }),
    ))
}

/// `GET /api/v2/users/@me/contracts`: the contracts this user is named in,
/// newest first, paged the way the application's own list is.
pub async fn mine(
    State(state): State<AppState>,
    user: Limited,
    RawQuery(raw): RawQuery,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let named = caller_discord_id(&state, &user).await?;

    let Some(named) = named else {
        // An account without a Discord id is nobody a contract can name, so it
        // is named in nothing.
        return Ok(Json(Value::Array(Vec::new())).into_response());
    };

    let params = QueryParams::parse(raw.as_deref().unwrap_or_default());
    let page = pagination::Page::asked(&params)?;

    let contracts = contract::of_party(state.pool(), named, page.cursor, page.limit)
        .await
        .map_err(contract_error)?;

    let next = page.next_cursor(&contracts, |contract| contract.id);

    Ok(paged(
        rendered(&contracts),
        next,
        page.limit,
        &headers,
        uri.path(),
    ))
}

/// `GET /api/v2/contracts/{id}/payments`: what this contract has paid out, newest
/// first.
///
/// The same readers as the contract itself: the application that wrote it and
/// the users it names, and nobody else — a caller who is neither is answered as
/// a contract that is not there.
///
/// **A row is a ledger entry, not a charge.** One payment draws on as many
/// parties as it needs — oldest approval first, or the one party it names — and
/// writes one row per party drawn on, so a statement of a multi-party contract
/// has several rows for one payment. For the one-party contract a metered
/// application writes the two are the same list.
pub async fn payments(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<i64>,
    RawQuery(raw): RawQuery,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let found = find(&state, id).await?;

    if !visible(&state, &user, &found).await? {
        return Err(ApiError::NotFound);
    }

    let params = QueryParams::parse(raw.as_deref().unwrap_or_default());
    let page = pagination::Page::asked(&params)?.limited_to(PAYMENTS_PER_PAGE);

    let entries = contract::payments(state.pool(), id, page.cursor, page.limit)
        .await
        .map_err(contract_error)?;

    let next = page.next_cursor(&entries, |payment| payment.id);
    let body = Value::Array(entries.iter().map(render_payment).collect());

    Ok(paged(body, next, page.limit, &headers, uri.path()))
}

fn render_payment(payment: &contract::Payment) -> Value {
    json!({
        "id": payment.id.to_string(),
        "discord_id": payment.discord_id.to_string(),
        "amount": payment.amount.to_string(),
        "receiver_discord_id": payment.receiver_discord_id.to_string(),
        "time": format_timestamp(payment.time),
    })
}

/// A list's answer: the body, and a `link` header when there is a next page.
///
/// The continuation is this family's own shape — `limit` and the last row's id
/// as `next` — because only the list knows which of its parameters the rest of
/// the query carries. For these three the rest is nothing: a contract list is
/// filtered by who is asking, not by parameters.
fn paged(
    body: Value,
    next: Option<i64>,
    limit: Option<i64>,
    headers: &HeaderMap,
    path: &str,
) -> Response {
    let mut response = Json(body).into_response();

    if let Some(next) = next {
        let query = format!("limit={}&next={next}", limit.unwrap_or_default());

        if let Some(value) = pagination::link(headers, path, &query) {
            response.headers_mut().insert("link", value);
        }
    }

    response
}

/// The body a creation arrives as, parsed rather than deserialized: which field
/// is missing or malformed is part of the answer, and a `serde` rejection would
/// answer it in the framework's words.
fn asked(body: &Value) -> Result<Asked, ApiError> {
    let Some(object) = body.as_object() else {
        return Err(ApiError::InvalidRequest("missing_parameter"));
    };

    let Some(unit) = object.get("unit").and_then(Value::as_str) else {
        return Err(ApiError::InvalidRequest("missing_parameter"));
    };

    let receiver_discord_id = match object.get("receiver_discord_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(discord_id)) => Some(parse_number(discord_id).ok_or(
            ApiError::InvalidRequest("invalid_format_of_receiver_discord_id"),
        )?),
        Some(_) => return Err(ApiError::InvalidRequest("invalid_type_of_variable")),
    };

    let expires_in = match object.get("expires_in") {
        None | Some(Value::Null) => None,
        Some(Value::Number(seconds)) => seconds.as_i64(),
        Some(_) => return Err(ApiError::InvalidRequest("invalid_type_of_variable")),
    };

    let Some(parties) = object.get("parties").and_then(Value::as_array) else {
        return Err(ApiError::InvalidRequest("missing_parameter"));
    };

    let mut named = Vec::with_capacity(parties.len());

    for (index, party) in parties.iter().enumerate() {
        let discord_id = party
            .get("discord_id")
            .and_then(Value::as_str)
            .and_then(parse_number)
            .ok_or(ApiError::BulkInvalid {
                tag: "discord_id",
                index,
            })?;

        let amount = party
            .get("amount")
            .and_then(Value::as_str)
            .and_then(parse_number)
            .ok_or(ApiError::BulkInvalid {
                tag: "amount",
                index,
            })?;

        named.push(NewParty { discord_id, amount });
    }

    Ok(Asked {
        unit: unit.to_owned(),
        parties: named,
        receiver_discord_id,
        expires_in,
    })
}

struct Asked {
    unit: String,
    parties: Vec<NewParty>,
    receiver_discord_id: Option<i64>,
    expires_in: Option<i64>,
}

/// The body a payment arrives as: who is paid, how much, and — when the contract
/// names more than one person — whose use it bills.
fn payment(body: &Value) -> Result<Payment, ApiError> {
    let Some(object) = body.as_object() else {
        return Err(ApiError::InvalidRequest("missing_parameter"));
    };

    let (Some(receiver), Some(amount)) = (
        object.get("receiver_discord_id").and_then(Value::as_str),
        object.get("amount").and_then(Value::as_str),
    ) else {
        return Err(ApiError::InvalidRequest("missing_parameter"));
    };

    // Absent, the draw is oldest-approval-first across every party, which is what
    // a contract that names one person means and what every payment meant before
    // this field existed.
    let party_discord_id = match object.get("party_discord_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(party)) => Some(parse_number(party).ok_or(ApiError::InvalidRequest(
            "invalid_format_of_party_discord_id",
        ))?),
        Some(_) => return Err(ApiError::InvalidRequest("invalid_type_of_variable")),
    };

    Ok(Payment {
        receiver_discord_id: parse_number(receiver).ok_or(ApiError::InvalidRequest(
            "invalid_format_of_receiver_discord_id",
        ))?,
        amount: parse_number(amount).ok_or(ApiError::InvalidRequest("invalid_format_of_amount"))?,
        party_discord_id,
    })
}

struct Payment {
    receiver_discord_id: i64,
    amount: i64,
    party_discord_id: Option<i64>,
}

/// The contract a client asked about, or a 404 — the domain's `None` is the same
/// nothing as an id that names nothing.
async fn find(state: &AppState, id: i64) -> Result<Contract, ApiError> {
    contract::find(state.pool(), id)
        .await
        .map_err(contract_error)?
        .ok_or(ApiError::NotFound)
}

fn rendered(contracts: &[Contract]) -> Value {
    Value::Array(contracts.iter().map(render).collect())
}

fn render(contract: &Contract) -> Value {
    json!({
        "id": contract.id.to_string(),
        "client_name": contract.client_name,
        "unit": contract.unit,
        "guild_id": contract.guild_id.map(|guild| guild.to_string()).unwrap_or_default(),
        "status": contract.status,
        "receiver_discord_id": contract.receiver_discord_id.map(|id| id.to_string()),
        "expires_at": contract.expires_at.map(format_timestamp),
        "remaining": contract.remaining.to_string(),
        "parties": contract
            .parties
            .iter()
            .map(|party| {
                json!({
                    "discord_id": party.discord_id.to_string(),
                    "amount": party.amount.to_string(),
                    "remaining": party.remaining.to_string(),
                    "status": party.status,
                })
            })
            .collect::<Vec<Value>>(),
    })
}

/// The application a caller is, which is what the application-side endpoints
/// need. The scope says it may ask, which is not the same as being allowed to
/// spend: what a contract holds is the parties' to give.
async fn application(state: &AppState, user: &AuthUser) -> Result<i64, ApiError> {
    if user.kind != vc_auth::Kind::App {
        return Err(ApiError::PermissionDenied);
    }

    if !user.scopes.vc_contract {
        return Err(ApiError::InsufficientScope);
    }

    let Ok(subject) = i32::try_from(user.subject) else {
        return Err(ApiError::Internal(
            "the token's subject is not an account id".into(),
        ));
    };

    vc_core::user::application_id(state.pool(), subject)
        .await
        .map_err(vc_core::Error::Database)?
        .ok_or(ApiError::PermissionDenied)
}

/// The account a user token is for, which is the identity a party is found by.
fn account(user: &AuthUser) -> Result<i32, ApiError> {
    if user.kind != vc_auth::Kind::User {
        return Err(ApiError::PermissionDenied);
    }

    i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("the token's subject is not an account id".into()))
}

async fn caller_discord_id(state: &AppState, user: &AuthUser) -> Result<Option<i64>, ApiError> {
    let account = account(user)?;
    let found = vc_core::user::find_by_id(state.pool(), account)
        .await?
        .ok_or(vc_core::Error::UserNotFound(user.subject))?;

    Ok(found.discord_id)
}

/// Whether this caller may see this contract: the application that wrote it, or
/// one of the users it names.
async fn visible(state: &AppState, user: &AuthUser, contract: &Contract) -> Result<bool, ApiError> {
    match user.kind {
        vc_auth::Kind::App => {
            let Ok(subject) = i32::try_from(user.subject) else {
                return Ok(false);
            };

            let application = vc_core::user::application_id(state.pool(), subject)
                .await
                .map_err(vc_core::Error::Database)?;

            Ok(application == Some(contract.application_id))
        }
        vc_auth::Kind::User => {
            let Some(named) = caller_discord_id(state, user).await? else {
                return Ok(false);
            };

            Ok(contract
                .parties
                .iter()
                .any(|party| party.discord_id == named))
        }
    }
}

/// What the domain's refusals are answered as: a request or a name that is wrong
/// is a description, a state that refused is an `error_info` — the same words
/// the claim endpoints use, in the shapes this family already answers with.
fn contract_error(error: ContractError) -> ApiError {
    match error {
        ContractError::NotFound => ApiError::NotFound,
        ContractError::NotFoundCurrency => ApiError::InvalidRequest("not_found_currency"),
        ContractError::InvalidAmount => ApiError::InvalidRequest("invalid_amount"),
        ContractError::InvalidParties => ApiError::InvalidRequest("invalid_parties"),
        ContractError::InvalidExpiresIn => ApiError::InvalidRequest("invalid_expires_in"),
        ContractError::NotEnoughAmount => ApiError::Conflict("not_enough_amount"),
        ContractError::InvalidStatus => ApiError::Conflict("invalid_status"),
        ContractError::Expired => ApiError::Conflict("expired"),
        ContractError::ReceiverIsFixed => ApiError::InvalidRequest("receiver_is_fixed"),
        ContractError::NotAParty => ApiError::InvalidRequest("not_a_party"),
        ContractError::Database(error) => ApiError::Core(vc_core::Error::Database(error)),
    }
}

/// Ids and amounts travel as strings because JSON's number cannot hold a
/// snowflake exactly; a string that is not a number is not one.
fn parse_number(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}
