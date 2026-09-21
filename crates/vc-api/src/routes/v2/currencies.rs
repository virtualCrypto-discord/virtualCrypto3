use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;
use vc_core::currency::{CurrencyInfo, CurrencySelector};

use crate::error::ApiError;
use crate::state::AppState;

/// The only parameters the controller looks at; anything else is ignored.
const PARAMETER_NAMES: [&str; 4] = ["id", "guild", "name", "unit"];

const NEED_ONE_PARAMETER: &str = "need_one_parameter_from_id_guild_name_or_unit";
const INVALID_ID: &str = "id_must_be_positive_integer";
const INVALID_GUILD_ID: &str = "guild_id_must_be_positive_integer";

#[derive(Serialize)]
struct CurrencyResponse {
    total_amount: String,
    name: Option<String>,
    unit: Option<String>,
    guild: String,
    pool_amount: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    error_description: String,
}

/// `GET /api/v2/currencies`
pub async fn index(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    respond(&state, &query, None, None).await
}

/// `GET /api/v2/currencies/:id` — the same action, with `id` supplied by the path.
///
/// It takes a token it does not require, which is why it is the one read that
/// rules on a grant: the currency read is public, and what it has to refuse is a
/// *grant* that named a currency outside its own reach. An account's own
/// credential — a session, a PAT, an application's `client_credentials` token —
/// is not a grant and is not narrowed, which [`crate::resource::of_bearer`] is
/// what tells apart.
pub async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let grant = crate::resource::of_bearer(
        &state,
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    )
    .await;

    respond(&state, &query, Some(id), grant).await
}

async fn respond(
    state: &AppState,
    query: &HashMap<String, String>,
    path_id: Option<String>,
    grant: Option<Vec<i64>>,
) -> Response {
    // Phoenix merges path parameters over query parameters, so a path id wins.
    let mut parameters = query.clone();
    if let Some(id) = path_id {
        parameters.insert("id".to_string(), id);
    }

    let supplied: Vec<(&str, &str)> = PARAMETER_NAMES
        .iter()
        .filter_map(|name| parameters.get(*name).map(|value| (*name, value.as_str())))
        .collect();

    if supplied.len() != 1 {
        return invalid_request(NEED_ONE_PARAMETER);
    }

    let (name, value) = supplied[0];
    let selector = match name {
        "id" => match positive_integer(value) {
            Some(id) => CurrencySelector::Id(id),
            None => return invalid_request(INVALID_ID),
        },
        "guild" => match positive_integer(value) {
            Some(guild_id) => CurrencySelector::Guild(guild_id),
            None => return invalid_request(INVALID_GUILD_ID),
        },
        "name" => CurrencySelector::Name(value),
        "unit" => CurrencySelector::Unit(value),
        _ => unreachable!("parameter names are constrained above"),
    };

    // A read that names a currency is refused like a write, because the request
    // asked for something the token is not worth — the same 403 an act outside a
    // grant gets. It is only the path form that names one here; the collection's
    // own query form is a public read and rules nothing.
    if let (Some(resources), CurrencySelector::Id(id)) = (&grant, &selector)
        && crate::resource::ensure(resources, *id).is_err()
    {
        return ApiError::InsufficientScope.into_response();
    }

    match vc_core::currency::info(state.pool(), selector).await {
        Ok(Some(info)) => Json(into_response(info)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "not_found".to_string(),
                error_description: "not_found".to_string(),
            }),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, "currency lookup failed");
            internal_server_error()
        }
    }
}

fn into_response(info: CurrencyInfo) -> CurrencyResponse {
    CurrencyResponse {
        total_amount: info.total_amount.to_string(),
        name: info.name,
        unit: info.unit,
        guild: info.guild_id.map(|id| id.to_string()).unwrap_or_default(),
        pool_amount: info
            .pool_amount
            .map(|amount| amount.to_string())
            .unwrap_or_default(),
    }
}

/// `Integer.parse/1` requires the whole string to be consumed and the value to be
/// at least 1.
fn positive_integer(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|parsed| *parsed >= 1)
}

fn invalid_request(description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "invalid_request".to_string(),
            error_description: description.to_string(),
        }),
    )
        .into_response()
}

fn internal_server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "errors": { "detail": "Internal Server Error" } })),
    )
        .into_response()
}
