//! `POST /api/v2/currencies/issue`: a guild's pool pays, on the authority of a
//! guild token.
//!
//! Not the Elixir's. `Authz.md` says a `guild` token is what may `give`, and
//! `Rest.md` documents no endpoint that does it — the Elixir had none, so the
//! guild tokens its code flow issued were never accepted anywhere. This is that
//! endpoint, and [`vc_core::grant::resolve_token`] is what makes the token mean
//! something: the grant it resolves to carries both the guild and the scope.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::routes::guild_token::GuildToken;
use crate::routes::idempotency::{self, Idempotency};
use crate::state::AppState;
use vc_core::issue::IssueError;

/// `POST /api/v2/currencies/issue`
///
/// The guild is the token's, so the path names none: a guild token can only ever
/// spend the pool of the guild it was issued for.
pub async fn post(
    State(state): State<AppState>,
    guild: GuildToken,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let claimed = match idempotency::claim(
        &state,
        &headers,
        guild.account_id,
        guild.scopes.vc_issue,
    )
    .await?
    {
        Idempotency::None => None,
        Idempotency::Claimed(key) => Some(key),
        Idempotency::Answered(response) => return Ok(response),
    };

    idempotency::answer(
        &state,
        claimed,
        guild.account_id,
        issued(&state, &guild, &body).await,
    )
    .await
}

/// The body, and the issue it asks for.
///
/// The shape is the payment endpoint's, because the two are the same kind of act:
/// two values, each a number written as a string, which is how this API carries
/// numbers.
async fn issued(
    state: &AppState,
    guild: &GuildToken,
    body: &Value,
) -> Result<(StatusCode, Value), ApiError> {
    let Some(object) = body.as_object() else {
        return Ok(missing_parameter());
    };

    let (Some(receiver), Some(amount)) = (object.get("receiver_discord_id"), object.get("amount"))
    else {
        return Ok(missing_parameter());
    };

    if !(receiver.is_string() && amount.is_string()) {
        return Ok((
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid_request", "error_description": "invalid_type_of_variable" }),
        ));
    }

    let receiver_discord_id = parse_number(receiver.as_str().unwrap_or_default()).ok_or(
        ApiError::InvalidRequest("invalid_format_of_receiver_discord_id"),
    )?;
    let amount = parse_number(amount.as_str().unwrap_or_default())
        .ok_or(ApiError::InvalidRequest("invalid_format_of_amount"))?;

    if !guild.scopes.vc_issue {
        return Err(ApiError::InsufficientScope);
    }

    // The amount is required where the command may leave it out. There an omitted
    // amount is `:all`, and the person who typed it is looking at the pool; here it
    // would be a bot that forgot a field draining the guild.
    match vc_core::issue::issue(
        state.pool(),
        guild.guild_id,
        receiver_discord_id,
        Some(amount),
    )
    .await
    {
        Ok(issued) => Ok((
            StatusCode::CREATED,
            json!({
                "amount": issued.amount.to_string(),
                "pool_amount": issued.pool_amount.to_string(),
                "unit": issued.unit,
            }),
        )),
        Err(error) => Ok(issue_error(error)),
    }
}

fn missing_parameter() -> (StatusCode, Value) {
    (
        StatusCode::BAD_REQUEST,
        json!({ "error": "invalid_request", "error_description": "missing_parameter" }),
    )
}

/// The payment endpoint's three shapes, for the three ways issuing fails.
///
/// The wording is the view's rather than the specification's — `Rest.md` puts
/// `error_description` outside the contract, and what a client matches is `error`
/// and `error_info` — and it is the same wording, so that one client library
/// reading both endpoints has one thing to learn.
fn issue_error(error: IssueError) -> (StatusCode, Value) {
    match error {
        IssueError::NotFoundCurrency => (
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid_request", "error_info": "not_found_currency" }),
        ),
        IssueError::NotEnoughAmount => (
            StatusCode::CONFLICT,
            json!({ "error": "conflict", "error_info": "not_enough_amount" }),
        ),
        IssueError::InvalidAmount => (
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid_request", "error_description": "invalid_amount" }),
        ),
        IssueError::Database(error) => {
            tracing::error!(%error, "issuing failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "errors": { "detail": "Internal Server Error" } }),
            )
        }
    }
}

fn parse_number(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}
