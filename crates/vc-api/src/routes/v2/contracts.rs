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
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use time::OffsetDateTime;

use vc_auth::AuthUser;

use crate::error::ApiError;
use crate::routes::limited::Limited;
use crate::routes::v2::claims::format_timestamp;
use crate::state::AppState;
use vc_core::contract::{self, Contract, ContractError, NewParty};

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

/// `GET /api/v2/contracts`: the ones this application wrote.
pub async fn index(State(state): State<AppState>, user: Limited) -> Result<Json<Value>, ApiError> {
    let application = application(&state, &user).await?;

    let contracts = contract::of_application(state.pool(), application)
        .await
        .map_err(contract_error)?;

    Ok(Json(rendered(&contracts)))
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
pub async fn approve(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let account = account(&user)?;

    contract::approve(state.pool(), id, account, OffsetDateTime::now_utc())
        .await
        .map_err(contract_error)?;

    Ok(Json(render(&find(&state, id).await?)))
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

    contract::refuse(state.pool(), id, account, OffsetDateTime::now_utc())
        .await
        .map_err(contract_error)?;

    Ok(Json(render(&find(&state, id).await?)))
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

    contract::withdraw(state.pool(), id, account, OffsetDateTime::now_utc())
        .await
        .map_err(contract_error)?;

    Ok(Json(render(&find(&state, id).await?)))
}

/// `POST /api/v2/contracts/{id}/payments`: the application spending what the
/// parties locked.
pub async fn pay(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let application = application(&state, &user).await?;
    let payment = payment(&body)?;

    let payed = contract::pay(
        state.pool(),
        id,
        application,
        payment.receiver_discord_id,
        payment.amount,
        OffsetDateTime::now_utc(),
    )
    .await
    .map_err(contract_error)?;

    let unit = find(&state, id).await?.unit;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "amount": payed.amount.to_string(),
            "remaining": payed.remaining.to_string(),
            "unit": unit,
        })),
    )
        .into_response())
}

/// `GET /api/v2/users/@me/contracts`: the contracts this user is named in.
pub async fn mine(State(state): State<AppState>, user: Limited) -> Result<Json<Value>, ApiError> {
    let named = caller_discord_id(&state, &user).await?;

    let Some(named) = named else {
        // An account without a Discord id is nobody a contract can name, so it
        // is named in nothing.
        return Ok(Json(Value::Array(Vec::new())));
    };

    let contracts = contract::of_party(state.pool(), named)
        .await
        .map_err(contract_error)?;

    Ok(Json(rendered(&contracts)))
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

    Ok(Payment {
        receiver_discord_id: parse_number(receiver).ok_or(ApiError::InvalidRequest(
            "invalid_format_of_receiver_discord_id",
        ))?,
        amount: parse_number(amount).ok_or(ApiError::InvalidRequest("invalid_format_of_amount"))?,
    })
}

struct Payment {
    receiver_discord_id: i64,
    amount: i64,
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
        ContractError::Database(error) => ApiError::Core(vc_core::Error::Database(error)),
    }
}

/// Ids and amounts travel as strings because JSON's number cannot hold a
/// snowflake exactly; a string that is not a number is not one.
fn parse_number(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}
