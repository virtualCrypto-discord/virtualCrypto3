use axum::Json;
use axum::extract::{OriginalUri, Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use time::PrimitiveDateTime;

use crate::routes::limited::Limited;
use crate::routes::pagination::{self, QueryParams, parse_number};
use vc_core::claim::{ClaimFilter, ClaimView, Order, SrFilter};

use crate::discord::filter_profile;
use crate::error::ApiError;
use crate::state::AppState;

const STATUSES: [&str; 4] = ["pending", "approved", "denied", "canceled"];

/// `GET /api/v2/users/@me/claims/:id`
///
/// Mirrors `VirtualCryptoWeb.Api.V2.ClaimController.get_by_id/2`: only the payer
/// or the claimant may read a claim, and the metadata is the requester's own.
pub async fn get_by_id(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if !user.scopes.vc_claim {
        return Err(ApiError::PermissionDenied);
    }

    let operator_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    // Ecto would cast the raw path segment into the bigint column and raise on a
    // non-numeric value; PATCH already answers those with 404, so do the same.
    let claim_id: i64 = id.parse().map_err(|_| ApiError::NotFound)?;

    let view = vc_core::claim::view(state.pool(), operator_id, claim_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if view.payer.id != operator_id && view.claimant.id != operator_id {
        return Err(ApiError::Forbidden("not_related_user"));
    }

    Ok(Json(serialize_claim(&state, view).await?))
}

/// `GET /api/v2/users/@me/claims`
///
/// Mirrors `ClaimController.me/2` and `Raw.Get.get_claims/7`: the status, side,
/// related-user, order, cursor and limit parameters are validated in the same
/// order, and the response carries a `link` header when a full page was returned.
pub async fn index(
    State(state): State<AppState>,
    user: Limited,
    RawQuery(raw): RawQuery,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !user.scopes.vc_claim {
        return Err(ApiError::PermissionDenied);
    }

    let operator_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    let params = QueryParams::parse(raw.as_deref().unwrap_or_default());

    // An absent or empty `statuses[]` means "pending"; anything else is rejected.
    let mut statuses = params.all("statuses[]");
    if statuses.is_empty() {
        statuses.push("pending".to_string());
    }
    if !statuses
        .iter()
        .all(|status| STATUSES.contains(&status.as_str()))
    {
        return Err(ApiError::InvalidRequest("invalid_statuses"));
    }

    let related_discord = params.one("related_discord_user_id");
    let related_vc = params.one("related_vc_user_id");
    let (related_param, related_user_id) = match (&related_discord, &related_vc) {
        (Some(_), Some(_)) => return Err(ApiError::InvalidRequest("invalid_related_user")),
        (Some(value), None) => {
            let discord_id = parse_related_id(value)?;
            (
                Some(("related_discord_user_id", value.clone())),
                Some(parse_related_discord(&state, discord_id).await?),
            )
        }
        (None, Some(value)) => {
            let user_id = parse_related_id(value)?;
            (Some(("related_vc_user_id", value.clone())), Some(user_id))
        }
        (None, None) => (None, None),
    };

    let type_param = params.one("type");
    let sr_filter = match type_param.as_deref() {
        None | Some("all") => SrFilter::All,
        Some("received") => SrFilter::Received,
        Some("claimed") => SrFilter::Claimed,
        Some(_) => return Err(ApiError::InvalidRequest("invalid_type")),
    };

    // A page whether or not the caller asked for one: the Elixir answered every
    // matching claim here, which is the one thing a list endpoint must not do.
    let page = pagination::Page::asked(&params)?.limited_to(pagination::PER_PAGE);

    let order_param = params.one("order");
    let order = match order_param.as_deref() {
        None | Some("desc_claim_id") => Order::Desc,
        Some("asc_claim_id") => Order::Asc,
        // An order that is not one of the two is the caller's mistake, so it is a
        // 400 with a name rather than the 500 the Elixir's `parse_order/1` crash
        // produced. `docs/known-gaps.md` records the difference.
        Some(_) => return Err(ApiError::InvalidRequest("invalid_order")),
    };

    let claims = vc_core::claim::list(
        state.pool(),
        ClaimFilter {
            operator_id,
            statuses: &statuses,
            sr_filter,
            related_user_id,
            order,
            cursor: page.cursor,
            limit: page.limit,
        },
    )
    .await?;

    let next = page.next_cursor(&claims, |claim| claim.id);
    let mut body = Vec::with_capacity(claims.len());
    for claim in claims {
        body.push(serialize_claim(&state, claim).await?);
    }

    let mut response = Json(Value::Array(body)).into_response();

    if let Some(next) = next {
        let query = pagination_query(
            type_param.as_deref().unwrap_or("all"),
            order_param.as_deref().unwrap_or("desc_claim_id"),
            next,
            page.limit.unwrap_or_default(),
            related_param
                .as_ref()
                .map(|(key, value)| (*key, value.as_str())),
            &statuses,
        );

        if let Some(value) = pagination::link(&headers, uri.path(), &query) {
            response.headers_mut().insert("link", value);
        }
    }

    Ok(response)
}

/// `POST /api/v2/users/@me/claims`
///
/// Mirrors `ClaimController.post/2`, whose clause order decides both which
/// `invalid_request` code a malformed body gets and which problem wins when
/// several apply:
///
/// 1. `payer_discord_id` and `amount` are strings → parse and create;
/// 2. payer present but not a string → `invalid_payer_discord_id_type`;
/// 3. amount present but not a string → `invalid_amount_type`;
/// 4. payer and unit present → `amount_field_is_required`;
/// 5. payer present → `unit_field_is_required`;
/// 6. otherwise → `payer_discord_id_field_is_required`.
///
/// Within clause 1 an unparseable payer is reported before an unparseable amount,
/// and a non-positive amount before any metadata validation.
pub async fn create(
    State(state): State<AppState>,
    user: Limited,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let object = body.as_object();
    let payer = object.and_then(|object| object.get("payer_discord_id"));
    let unit = object.and_then(|object| object.get("unit"));
    let amount = object.and_then(|object| object.get("amount"));

    // Clause 1. The guard only checks the payer and the amount, so a non-string
    // `unit` still reaches the currency lookup and simply fails to match.
    if let (Some(payer), Some(amount)) = (payer, amount)
        && payer.is_string()
        && amount.is_string()
    {
        if !user.scopes.vc_claim {
            return Err(ApiError::PermissionDenied);
        }

        let operator_id = i32::try_from(user.subject)
            .map_err(|_| ApiError::Internal("subject out of range".into()))?;

        let payer_discord_id = parse_number(payer.as_str().unwrap_or_default())
            .ok_or(ApiError::InvalidRequest("invalid_payer_discord_id_value"))?;
        let amount_value = parse_number(amount.as_str().unwrap_or_default())
            .ok_or(ApiError::InvalidRequest("invalid_amount_value"))?;

        // `create_claim/5`'s guard runs before any metadata validation.
        if amount_value <= 0 {
            return Err(ApiError::InvalidRequest("invalid_amount"));
        }

        let metadata = object
            .and_then(|object| object.get("metadata"))
            .filter(|value| !value.is_null())
            .cloned();

        let details = metadata
            .as_ref()
            .map(vc_core::metadata::validate)
            .unwrap_or_default();
        if !details.is_empty() {
            return Err(ApiError::InvalidMetadata(details));
        }

        let unit = unit
            .and_then(Value::as_str)
            .ok_or(ApiError::InvalidRequest("not_found_currency"))?;

        let claim_id = vc_core::claim::create(
            state.pool(),
            operator_id,
            payer_discord_id,
            unit,
            amount_value,
            metadata,
        )
        .await
        .map_err(create_error)?;

        let view = vc_core::claim::view(state.pool(), operator_id, claim_id)
            .await?
            .ok_or(ApiError::NotFound)?;

        return Ok((
            StatusCode::CREATED,
            Json(serialize_claim(&state, view).await?),
        )
            .into_response());
    }

    // Clause 2.
    if let Some(payer) = payer
        && !payer.is_string()
    {
        return Err(ApiError::InvalidRequest("invalid_payer_discord_id_type"));
    }

    // Clause 3.
    if let Some(amount) = amount
        && !amount.is_string()
    {
        return Err(ApiError::InvalidRequest("invalid_amount_type"));
    }

    // Clause 4.
    if payer.is_some() && unit.is_some() {
        return Err(ApiError::InvalidRequest("amount_field_is_required"));
    }

    // Clause 5.
    if payer.is_some() {
        return Err(ApiError::InvalidRequest("unit_field_is_required"));
    }

    // Clause 6.
    Err(ApiError::InvalidRequest(
        "payer_discord_id_field_is_required",
    ))
}

fn create_error(error: vc_core::claim::CreateError) -> ApiError {
    use vc_core::claim::CreateError;

    match error {
        CreateError::InvalidAmount => ApiError::InvalidRequest("invalid_amount"),
        CreateError::NotFoundCurrency => ApiError::InvalidRequest("not_found_currency"),
        CreateError::Database(error) => ApiError::Core(vc_core::Error::Database(error)),
    }
}

/// `PATCH /api/v2/users/@me/claims/:id`
///
/// Mirrors `ClaimController.patch/2`. The clause order matters:
///
/// 1. a recognised `status` performs that transition;
/// 2. otherwise a `metadata` field performs a metadata-only update, even when
///    `status` was present but unrecognised;
/// 3. otherwise the request is rejected as `must_supply_valid_status`.
///
/// For a transition, `Map.get(d, "metadata", %{})` means an absent field becomes
/// `{}` (merge, preserving existing metadata) while an explicit `null` deletes —
/// and transitions do not run the application-side metadata validator, so only
/// the database trigger can reject their metadata.
pub async fn patch(
    State(state): State<AppState>,
    user: Limited,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    if !user.scopes.vc_claim {
        return Err(ApiError::PermissionDenied);
    }

    let operator_id = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    // `Integer.parse(id)` with a clean tail; anything else is not found.
    let claim_id: i64 = id.parse().map_err(|_| ApiError::NotFound)?;

    let object = body.as_object();
    let status = object
        .and_then(|object| object.get("status"))
        .and_then(Value::as_str);
    let has_metadata = object.is_some_and(|object| object.contains_key("metadata"));
    // JSON null is Elixir's `nil`: it means "delete the metadata", not "store null".
    let metadata = object
        .and_then(|object| object.get("metadata"))
        .filter(|value| !value.is_null())
        .cloned();

    let transition = match status {
        Some("approved") => Some(vc_core::claim::Transition::Approved),
        Some("denied") => Some(vc_core::claim::Transition::Denied),
        Some("canceled") => Some(vc_core::claim::Transition::Canceled),
        _ => None,
    };

    if let Some(transition) = transition {
        let metadata = if has_metadata {
            metadata
        } else {
            Some(json!({}))
        };

        vc_core::claim::transition(
            state.pool(),
            state.notifier(),
            operator_id,
            claim_id,
            transition,
            metadata,
        )
        .await
        .map_err(transition_error)?;
    } else if has_metadata {
        let details = metadata
            .as_ref()
            .map(vc_core::metadata::validate)
            .unwrap_or_default();

        if !details.is_empty() {
            return Err(ApiError::InvalidMetadata(details));
        }

        vc_core::claim::set_metadata(state.pool(), operator_id, claim_id, metadata)
            .await
            .map_err(transition_error)?;
    } else {
        return Err(ApiError::InvalidRequest("must_supply_valid_status"));
    }

    let view = vc_core::claim::view(state.pool(), operator_id, claim_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(serialize_claim(&state, view).await?))
}

fn transition_error(error: vc_core::claim::TransitionError) -> ApiError {
    use vc_core::claim::TransitionError;

    match error {
        TransitionError::NotFound => ApiError::NotFound,
        TransitionError::InvalidStatus => ApiError::Conflict("invalid_status"),
        TransitionError::InvalidOperator => ApiError::Forbidden("invalid_operator"),
        TransitionError::NotEnoughAmount | TransitionError::NotFoundSenderAsset => {
            ApiError::Conflict("not_enough_amount")
        }
        TransitionError::NotFoundCurrency => ApiError::InvalidRequest("not_found_currency"),
        TransitionError::InvalidAmount => ApiError::InvalidRequest("invalid_amount"),
        TransitionError::MetadataLimit => ApiError::MetadataLimit,
        TransitionError::Database(error) => ApiError::Core(vc_core::Error::Database(error)),
    }
}

/// `format_claim/2`: amounts and ids as strings, timestamps as UTC with a `Z`,
/// and the claimant/payer decorated with their filtered Discord profile.
pub async fn serialize_claim(state: &AppState, view: ClaimView) -> Result<Value, ApiError> {
    let claimant_discord = discord_user(state, view.claimant.discord_id).await?;
    let payer_discord = discord_user(state, view.payer.discord_id).await?;

    Ok(json!({
        "id": view.id.to_string(),
        "currency": {
            "name": view.currency.name,
            "unit": view.currency.unit,
            "guild": view.currency.guild_id.map(|id| id.to_string()).unwrap_or_default(),
            "pool_amount": view.currency.pool_amount.map(|amount| amount.to_string()).unwrap_or_default(),
        },
        "amount": view.amount.map(|amount| amount.to_string()).unwrap_or_default(),
        "claimant": {
            "id": view.claimant.id.to_string(),
            "discord": claimant_discord,
        },
        "payer": {
            "id": view.payer.id.to_string(),
            "discord": payer_discord,
        },
        "created_at": format_timestamp(view.inserted_at),
        "updated_at": format_timestamp(view.updated_at),
        "status": view.status,
        "metadata": view.metadata,
    }))
}

async fn discord_user(state: &AppState, discord_id: Option<i64>) -> Result<Value, ApiError> {
    let Some(discord_id) = discord_id else {
        return Ok(Value::Null);
    };

    match state.discord().get_user(discord_id).await? {
        Some(payload) => Ok(Value::Object(filter_profile(payload).into_iter().collect())),
        // `Filtering.user/1` is called on the cached `:not_found` atom and raises.
        None => Err(ApiError::Internal(format!(
            "discord user {discord_id} not found"
        ))),
    }
}

/// Related-user IDs must be positive ASCII decimal IDs, without a sign.
fn parse_related_id(value: &str) -> Result<i64, ApiError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::InvalidRequest("invalid_related_user"));
    }
    parse_number(value)
        .filter(|id| *id > 0)
        .ok_or(ApiError::InvalidRequest("invalid_related_user"))
}

/// `related_discord_user_id` is resolved to a virtualCrypto user, which is what
/// the claim filter needs. Elixir's resolver may create the user; a lookup is
/// enough here because a user without claims matches nothing either way.
async fn parse_related_discord(state: &AppState, discord_id: i64) -> Result<i64, ApiError> {
    let user = vc_core::user::find_by_discord_id(state.pool(), discord_id).await?;

    // An unregistered user has no claims. Keep the filter (rather than
    // removing it), but match no account, as the Discord claim list does.
    Ok(user.map_or(-1, |user| i64::from(user.id)))
}

/// `build_url_from_options/2`: the query is rebuilt from the options, in this
/// order, with `statuses[]` percent-encoded the way `URI.encode_query/1` does.
fn pagination_query(
    type_param: &str,
    order_param: &str,
    next: i64,
    limit: i64,
    related: Option<(&str, &str)>,
    statuses: &[String],
) -> String {
    let mut parts = vec![
        format!("type={type_param}"),
        format!("order={order_param}"),
        format!("next={next}"),
        format!("limit={limit}"),
    ];

    if let Some((key, value)) = related {
        parts.push(format!("{key}={value}"));
    }

    for status in statuses {
        parts.push(format!("statuses%5B%5D={status}"));
    }

    parts.join("&")
}

/// `DateTime.from_naive!(naive, "Etc/UTC")` serialized by Jason.
pub(crate) fn format_timestamp(value: PrimitiveDateTime) -> String {
    let format =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

    value.format(&format).unwrap_or_default()
}
