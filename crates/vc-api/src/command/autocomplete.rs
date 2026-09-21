//! `Interaction.AutoComplete`: the suggestions an option offers while it is
//! being typed, answered as a type 8 result.
//!
//! Elixir has no test for this, so nothing here is a port; it follows
//! `AutoComplete.handle/5` and the four searches behind it.

use serde_json::{Value, json};
use vc_core::claim::{ClaimUser, SrFilter};

use super::{CommandError, as_int, get_user, value_text};
use crate::state::AppState;

/// `AutoComplete.ClaimId` asks for at most this many, and Discord shows fewer.
const LIMIT: i64 = 25;

/// Which of a currency's two names an option is asking for.
#[derive(Clone, Copy)]
enum Field {
    Unit,
    Name,
}

/// `AutoComplete.handle/5`. `path` is the command name plus its subcommand when
/// it has one, because the same `id` option means different claims depending on
/// which command asked.
pub async fn handle(
    state: &AppState,
    path: &[&str],
    focused: &Value,
    payload: &Value,
) -> Result<Value, CommandError> {
    let me = get_user(payload).ok_or_else(|| CommandError::missing("interaction has no user"))?;
    let guild_id = payload.get("guild_id").and_then(as_int);
    let query = value_text(focused.get("value").unwrap_or(&Value::Null));
    let name = focused
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let choices = match (name, path) {
        ("unit", _) => currencies(state, &query, guild_id, me, Field::Unit).await?,
        ("name", _) => currencies(state, &query, guild_id, me, Field::Name).await?,
        ("id", ["claim", subcommand]) if ["approve", "deny", "cancel"].contains(subcommand) => {
            // Approving and denying are the payer's, cancelling the claimant's,
            // and either way only a pending claim can still move.
            let filter = if *subcommand == "cancel" {
                SrFilter::Claimed
            } else {
                SrFilter::Received
            };

            claims(state, &query, guild_id, me, filter, &["pending"]).await?
        }
        ("id", ["claim", "show"]) => {
            claims(
                state,
                &query,
                guild_id,
                me,
                SrFilter::All,
                &["pending", "approved", "denied", "canceled"],
            )
            .await?
        }
        // Not from the Elixir: neither site had a Discord surface for applications, so
        // there is no autocomplete to port. The path is the command and its subcommand, so
        // every `/application` subcommand that takes a `client_id` comes through here.
        ("client_id", ["application", _]) => applications(state, &query, me).await?,
        // `/help command:<名前>`: the commands that exist, which is the list the
        // screens are built from rather than a second copy of the names.
        ("command", ["help"]) => commands(&query),
        // `AutoComplete.handle/5` has no clause for anything else, so it raises.
        _ => {
            return Err(CommandError::missing(
                "nothing autocompletes for this option",
            ));
        }
    };

    Ok(json!({
        "type": super::AUTOCOMPLETE_RESULT,
        "data": { "choices": choices },
    }))
}

/// The commands `/help` can show, as choices.
///
/// The name is both the label and the value: what the screen is looked up by is
/// the name, and a second word for it here would be a second thing to keep in
/// step. The list comes from [`crate::docs::showings`], so a command cannot be
/// registered and unofferable.
fn commands(query: &str) -> Vec<Value> {
    crate::docs::showings()
        .into_iter()
        .filter(|showing| query.is_empty() || showing.name.contains(query))
        .take(LIMIT as usize)
        .map(|showing| json!({ "name": showing.name, "value": showing.name }))
        .collect()
}

/// The caller's own applications, as choices.
///
/// `resolve_discord_id` and `owned_by` are the two reads the client list already uses, and
/// a person may own several, so the label carries the name and the uuid both: a name is
/// not unique and the uuid is what every later call is made with.
async fn applications(state: &AppState, query: &str, me: i64) -> Result<Vec<Value>, CommandError> {
    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    let mut choices = Vec::new();

    for application_id in vc_core::application::owned_by(state.pool(), account).await? {
        let found = crate::routes::oauth2_clients::details(state.pool(), application_id)
            .await
            .map_err(vc_core::Error::from)?;

        let Some(found) = found else {
            continue;
        };

        let label = format!(
            "{}（{}）",
            found.client_name.unwrap_or_default(),
            found.client_id
        );

        // Discord filters nothing: it shows what it is given, so the query is applied here.
        if !query.is_empty() && !label.contains(query) && !found.client_id.starts_with(query) {
            continue;
        }

        choices.push(json!({
            "name": truncate(&label, 100),
            "value": found.client_id,
        }));

        if choices.len() as i64 >= LIMIT {
            break;
        }
    }

    Ok(choices)
}

/// Discord counts a choice's name in characters, and a `client_name` may be Japanese.
fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// `CurrencyUnit` and `CurrencyName`, which differ only in the field they match
/// and offer back.
async fn currencies(
    state: &AppState,
    query: &str,
    guild_id: Option<i64>,
    me: i64,
    field: Field,
) -> Result<Vec<Value>, CommandError> {
    let account = vc_core::user::resolve_discord_id(state.pool(), me).await?;

    // An empty query is the whole of what the caller could pick from, which is
    // their own currencies and their guild's.
    let candidates = if query.is_empty() {
        vc_core::currency::search_by_guild_and_user(state.pool(), guild_id, account, LIMIT).await?
    } else {
        match field {
            Field::Unit => {
                vc_core::currency::search_by_unit(state.pool(), query, guild_id, account, LIMIT)
                    .await?
            }
            Field::Name => {
                vc_core::currency::search_by_name(state.pool(), query, guild_id, account, LIMIT)
                    .await?
            }
        }
    };

    Ok(candidates
        .into_iter()
        .map(|candidate| {
            let unit = candidate.unit.clone().unwrap_or_default();

            json!({
                "name": format!(
                    "通貨名: {} 所持量: {}{}",
                    candidate.name.clone().unwrap_or_default(),
                    candidate.amount,
                    unit,
                ),
                "value": match field {
                    Field::Unit => unit,
                    Field::Name => candidate.name.unwrap_or_default(),
                },
            })
        })
        .collect())
}

/// `AutoComplete.ClaimId`: the claims the caller could act on from where they
/// are, named the way the option reads.
async fn claims(
    state: &AppState,
    query: &str,
    guild_id: Option<i64>,
    me: i64,
    filter: SrFilter,
    statuses: &[&str],
) -> Result<Vec<Value>, CommandError> {
    let statuses: Vec<String> = statuses
        .iter()
        .map(|status| (*status).to_string())
        .collect();

    let found = vc_core::claim::search_candidates(
        state.pool(),
        me,
        query,
        filter,
        &statuses,
        guild_id,
        LIMIT,
    )
    .await?;

    let mut choices = Vec::with_capacity(found.len());

    for claim in found {
        let claimant = party(state, &claim.claimant).await?;
        let payer = party(state, &claim.payer).await?;

        choices.push(json!({
            "name": format!(
                "{}  請求id: {}  金額: {}{}  請求元: {}  請求先: {}",
                status_emoji(claim.status.as_deref()),
                claim.id,
                claim.amount.unwrap_or_default(),
                claim.currency.unit.clone().unwrap_or_default(),
                claimant,
                payer,
            ),
            "value": claim.id.to_string(),
        }));
    }

    Ok(choices)
}

/// `ClaimId.user_tag/1`: a discord name, the four-digit form when there is one,
/// and `deleted` when Discord does not know them.
///
/// An application account is named by its registered client name, as in Elixir.
async fn party(state: &AppState, user: &ClaimUser) -> Result<String, CommandError> {
    let Some(discord_id) = user.discord_id else {
        if let Some(application_id) = vc_core::user::application_id(state.pool(), user.id).await?
            && let Some(application) =
                crate::routes::oauth2_clients::details(state.pool(), application_id).await?
        {
            return Ok(format!(
                "{}(app)",
                application.client_name.unwrap_or_default()
            ));
        }
        return Ok("deleted".to_string());
    };

    let Ok(Some(profile)) = state.discord().get_user(discord_id).await else {
        return Ok("deleted".to_string());
    };

    let name = profile
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or("deleted");

    Ok(match profile.get("discriminator").and_then(Value::as_str) {
        Some("0") | None => name.to_string(),
        Some(discriminator) => format!("{name}#{discriminator}"),
    })
}

/// `ClaimId.claim_status_emoji/1`.
fn status_emoji(status: Option<&str>) -> &'static str {
    match status {
        Some("approved") => "✅",
        Some("denied") => "❌",
        Some("canceled") => "🗑️",
        Some("pending") => "⌛",
        _ => "",
    }
}
