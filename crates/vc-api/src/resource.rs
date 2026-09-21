//! Which currencies an ask or a token is for: RFC 8707's `resource`.
//!
//! `docs/resources.md` is the design and RFC 8707 is the specification it is
//! taken from. A `resource` value names a protected resource as an absolute URI,
//! and the two this service has are its own path to a currency:
//! `/api/v2/currencies/{id}` for one currency, `/api/v2/currencies` for all of
//! them.
//!
//! What is *stored* is the currency id, never the URI: the URI is the wire form,
//! and an id is what keeps a grant working when the site moves. A collection URI
//! — and an ask that names nothing at all — therefore both resolve to the empty
//! set, which is how "every currency of the target, now and later" is written
//! everywhere else, and is also what a grant written before any of this existed
//! carries.
//!
//! The one function the endpoints share is [`ensure`]: it answers whether a
//! currency a request names is inside what the token is for.

use sqlx::PgPool;

use crate::error::ApiError;
use crate::state::AppState;
use vc_core::grant::Target;

/// What one `resource` value named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    /// `/api/v2/currencies` — every currency of the target, now and later. The
    /// same fact as naming none, so it writes no row.
    Collection,
    /// `/api/v2/currencies/{id}` — one currency, by its own id.
    Currency(i64),
}

/// A `resource` that is not a resource of this service, in RFC 8707's own word
/// for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTarget;

impl From<InvalidTarget> for ApiError {
    fn from(_: InvalidTarget) -> Self {
        ApiError::InvalidTarget
    }
}

/// Where a site lives, as an origin to compare a `resource` URI against:
/// `scheme://host[:port]`, with no path.
fn origin(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }

    Some(url.origin().ascii_serialization())
}

/// What one value names, or [`InvalidTarget`].
///
/// This is the "absolute, a URI, and this service's own path" half of the rule;
/// whether the id is one *this ask* may name is [`resolve`]'s, because that is a
/// question about the database rather than about the string.
pub fn parse(site_url: &str, value: &str) -> Result<Named, InvalidTarget> {
    let url = reqwest::Url::parse(value).map_err(|_| InvalidTarget)?;

    // RFC 8707: an absolute URI, and one that carries no fragment component.
    if !matches!(url.scheme(), "http" | "https") || url.fragment().is_some() {
        return Err(InvalidTarget);
    }

    if Some(url.origin().ascii_serialization()) != origin(site_url) {
        return Err(InvalidTarget);
    }

    let Some(rest) = url.path().strip_prefix("/api/v2/currencies") else {
        return Err(InvalidTarget);
    };

    if rest.is_empty() {
        return Ok(Named::Collection);
    }

    let Some(id) = rest.strip_prefix('/') else {
        return Err(InvalidTarget);
    };

    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(InvalidTarget);
    }

    id.parse::<i64>()
        .ok()
        .filter(|id| *id >= 1)
        .map(Named::Currency)
        .ok_or(InvalidTarget)
}

/// The guild a currency belongs to, or `None` for an id no currency has. The
/// inner `Option` is the column: a currency is a guild's, but the schema says
/// so with a nullable column.
async fn currency_guild(pool: &PgPool, id: i64) -> Result<Option<Option<i64>>, sqlx::Error> {
    sqlx::query_scalar!("SELECT guild_id FROM currencies WHERE id = $1", id)
        .fetch_optional(pool)
        .await
}

/// Resolve an ask's `resource` values to the currency ids they name, checking
/// each is a resource of this service and, for a guild's ask, a currency of that
/// guild.
///
/// The collection form contributes nothing, which is exactly the "every
/// currency" fact: an ask that names the collection and one that names nothing
/// resolve to the same empty set. Repeats are collapsed and the caller's order
/// is kept, which is what `check_scopes` does to a scope list for the same
/// reason.
pub async fn resolve(
    pool: &PgPool,
    site_url: &str,
    target: Target,
    values: &[String],
) -> Result<Vec<i64>, InvalidTarget> {
    let mut ids: Vec<i64> = Vec::new();

    for value in values {
        let Named::Currency(id) = parse(site_url, value)? else {
            continue;
        };

        if ids.contains(&id) {
            continue;
        }

        let Some(guild) = currency_guild(pool, id).await.map_err(|_| InvalidTarget)? else {
            return Err(InvalidTarget);
        };

        // A guild's ask is about that guild's pool, so a currency of another
        // guild is not a resource *it* has to offer — RFC 8707's §3 reason, and
        // the same question the database's one-currency-per-guild rule answers.
        if let Target::Guild(guild_id) = target
            && guild != Some(guild_id)
        {
            return Err(InvalidTarget);
        }

        ids.push(id);
    }

    Ok(ids)
}

/// Whether a grant that names these currencies covers this one.
///
/// An empty set is every currency of the target, which is what the collection
/// form, an omitted `resource` and a grant written before this existed all mean.
pub fn covers(resources: &[i64], currency_id: i64) -> bool {
    resources.is_empty() || resources.contains(&currency_id)
}

/// The currency a unit names.
///
/// A unit is unique across every guild here (`info_unit_index`), which is why a
/// request that carries only a unit can still be ruled on: it names one currency
/// or none.
pub async fn id_for_unit(pool: &PgPool, unit: &str) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!("SELECT id FROM currencies WHERE unit = $1", unit)
        .fetch_optional(pool)
        .await
}

/// The guild's currency, which is the one a guild's issue lands on.
///
/// `None` is a guild with no currency at all: there is nothing to be outside the
/// grant, so the check is [`ensure`]'s own "covers everything" and the act itself
/// is left to refuse it as `not_found_currency`.
pub async fn id_for_guild(pool: &PgPool, guild_id: i64) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!("SELECT id FROM currencies WHERE guild_id = $1", guild_id)
        .fetch_optional(pool)
        .await
}

/// The currency a claim's own row is in, which is the currency deciding it acts
/// on. `None` is a claim that is not there: the transition that follows is what
/// refuses that, not this.
pub async fn id_for_claim(pool: &PgPool, claim_id: i64) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!("SELECT currency_id FROM claims WHERE id = $1", claim_id)
        .fetch_optional(pool)
        .await
}

/// The units a grant's currencies are named by, for a screen that has to filter
/// or name them — or `None` for the empty set, which is every currency of the
/// target. The distinction is the one [`covers`] draws: an empty grant has no
/// list to filter by, and a filter of "none" would hide everything.
pub async fn covered_units(
    pool: &PgPool,
    resources: &[i64],
) -> Result<Option<Vec<String>>, sqlx::Error> {
    if resources.is_empty() {
        return Ok(None);
    }

    Ok(Some(units(pool, resources).await?))
}

/// Whether a currency named by `unit` is inside the grant. `None` is every
/// currency, which a filtered list reads as "do not filter".
pub fn covers_unit(covered: Option<&[String]>, unit: Option<&str>) -> bool {
    match covered {
        None => true,
        Some(units) => unit.is_some_and(|unit| units.iter().any(|candidate| candidate == unit)),
    }
}

/// The units these currencies are named by, in the order given, for a screen
/// that has to name them rather than their ids.
///
/// A currency whose `unit` is null — which the schema allows, though nothing
/// creates one — is named by its id instead, so a screen never has a blank where
/// a currency should be.
pub async fn units(pool: &PgPool, ids: &[i64]) -> Result<Vec<String>, sqlx::Error> {
    let mut units = Vec::with_capacity(ids.len());

    for id in ids {
        let unit = sqlx::query_scalar!("SELECT unit FROM currencies WHERE id = $1", id)
            .fetch_optional(pool)
            .await?
            .flatten();

        units.push(unit.unwrap_or_else(|| id.to_string()));
    }

    Ok(units)
}

/// Refuse an act that lands on a currency the token is not for.
///
/// `403 insufficient_scope` is the answer because the token is a real one, for
/// something else — the same answer a token without the scope gets.
pub fn ensure(resources: &[i64], currency_id: i64) -> Result<(), ApiError> {
    if covers(resources, currency_id) {
        Ok(())
    } else {
        Err(ApiError::InsufficientScope)
    }
}

/// [`ensure`] for an act that names its currency by a unit: the unit is resolved
/// to the currency it names and the same check is made.
///
/// A unit no currency has resolves to nothing, and is left to the act itself to
/// refuse as `not_found_currency` — the check here is about which currencies a
/// token is worth, not about whether a currency exists.
pub async fn ensure_unit(pool: &PgPool, resources: &[i64], unit: &str) -> Result<(), ApiError> {
    match id_for_unit(pool, unit)
        .await
        .map_err(vc_core::Error::from)?
    {
        Some(id) => ensure(resources, id),
        None => Ok(()),
    }
}

/// [`ensure`] for an act that lands on a guild's own currency, which a guild
/// token's issue is.
pub async fn ensure_guild(pool: &PgPool, resources: &[i64], guild_id: i64) -> Result<(), ApiError> {
    match id_for_guild(pool, guild_id)
        .await
        .map_err(vc_core::Error::from)?
    {
        Some(id) => ensure(resources, id),
        None => Ok(()),
    }
}

/// [`ensure`] for an act that lands on the currency a claim's own row names.
pub async fn ensure_claim(pool: &PgPool, resources: &[i64], claim_id: i64) -> Result<(), ApiError> {
    match id_for_claim(pool, claim_id)
        .await
        .map_err(vc_core::Error::from)?
    {
        Some(id) => ensure(resources, id),
        None => Ok(()),
    }
}

/// What a bearer token is narrowed to, when it is a grant's token at all.
///
/// `None` is every credential that is the account's own — a browser session, a
/// PAT, an application's own `client_credentials` token — and every token that
/// is not a grant's. None of those is narrowed, which is the whole of the
/// difference `docs/resources.md` draws. `Some` is a grant token, holding the
/// currencies it was approved for.
///
/// This is for the one endpoint that takes a token without requiring one: the
/// currency read is public, and what it has to refuse is a *grant* that named a
/// currency outside its own reach.
pub async fn of_bearer(state: &AppState, header: Option<&str>) -> Option<Vec<i64>> {
    let token = header.and_then(vc_auth::extractor::bearer_token)?;
    let token_id = uuid::Uuid::parse_str(token).ok()?;

    let resolved =
        vc_core::grant::resolve_token(state.pool(), token_id, time::OffsetDateTime::now_utc())
            .await
            .ok()??;

    Some(resolved.resources)
}
