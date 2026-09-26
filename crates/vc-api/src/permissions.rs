//! Discord permissions, as far as this service asks about them.
//!
//! An interaction arrives with the caller's permissions already computed by
//! Discord, which is why the commands need only
//! [`crate::command::is_administrator`] applied to a number. A consent screen is
//! a REST lookup instead, so the number has to be built here — the Elixir ORs
//! together the permissions of the roles the member carries.

use serde_json::{Map, Value};

use crate::command::{as_int, as_permissions, is_administrator};
use crate::state::AppState;

/// What the consent screen needs to know about a guild and one member of it,
/// read out of Discord's own JSON.
///
/// All three of these numbers arrive as *strings*: a guild's `owner_id`, a
/// member's `roles`, and a role's `permissions` — a bit set that needs all
/// sixty-four bits, which is why it is read as a `u64`. That is what makes this
/// worth being a function with tests rather than three lines inside a handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildFacts {
    pub owner_id: i64,
    pub member_role_ids: Vec<i64>,
    pub roles: Vec<(i64, u64)>,
}

/// Read them, or nothing if Discord answered with something unreadable.
///
/// `None` here is not "not a member" — a member who is not there is `None` from
/// the lookup that produced these. This is a payload that does not make sense,
/// and the caller answers it as a bad request.
pub fn guild_facts(
    guild: &Map<String, Value>,
    member: &Map<String, Value>,
    roles: &[Map<String, Value>],
) -> Option<GuildFacts> {
    let owner_id = guild.get("owner_id").and_then(as_int)?;

    let member_role_ids = member
        .get("roles")
        .and_then(Value::as_array)?
        .iter()
        .map(as_int)
        .collect::<Option<Vec<i64>>>()?;

    let roles = roles
        .iter()
        .map(|role| {
            Some((
                as_int(role.get("id")?)?,
                as_permissions(role.get("permissions")?)?,
            ))
        })
        .collect::<Option<Vec<(i64, u64)>>>()?;

    Some(GuildFacts {
        owner_id,
        member_role_ids,
        roles,
    })
}

/// `validate_executor`'s question: may this account act for the guild?
///
/// The account id is a **Discord** id. The Elixir passed the session's
/// VirtualCrypto user id here instead, which is why its answer was always no —
/// see the deliberate differences in docs/known-gaps.md.
pub fn may_act_for_guild(account_discord_id: i64, facts: &GuildFacts) -> bool {
    account_discord_id == facts.owner_id
        || is_administrator(member_permissions(&facts.member_role_ids, &facts.roles))
}

/// The permissions a member has: the permissions of their roles, ORed together.
///
/// A role id the guild does not have is skipped, which is a **fix** and not a
/// defensive flourish. The Elixir indexes the guild's roles with each of the
/// member's ids and calls `String.to_integer/1` on whatever comes back, so an id
/// that is not there raises.
///
/// That is reachable rather than theoretical: the member and the guild's roles
/// come from two separate Discord calls, and a role deleted between them lands
/// exactly here. The Elixir's answer is a 500 for the consent screen; this one is
/// a permission check that may be a moment stale.
pub fn member_permissions(member_role_ids: &[i64], roles: &[(i64, u64)]) -> u64 {
    member_role_ids
        .iter()
        .filter_map(|id| {
            roles
                .iter()
                .find(|(role, _)| role == id)
                .map(|(_, permissions)| *permissions)
        })
        .fold(0, |held, permissions| held | permissions)
}

/// What asking Discord about a guild and an account came to.
///
/// Three answers rather than two, because the surfaces that ask need to tell
/// "there is nothing here to ask about" from "the person asking may not": the
/// consent screen redirects the browser to the client for the first and refuses
/// for the second, and an API answers 404 and 403.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuildAccess {
    /// The account owns the guild, or administers it.
    Permitted,
    /// Discord has no such guild, this service's bot is not in it, or the account
    /// is not: a request about it cannot be answered at all.
    Unknown,
    /// The account is in the guild and may not act for it.
    Denied,
}

/// May this account act for this guild?
///
/// The chain the consent screen has always run, in one place because three
/// surfaces ask the same question — the consent screen, the page that allows a
/// guild, and the connect flow's own variant — and because the answer is the same
/// one each time. A Discord lookup that fails is [`GuildAccess::Unknown`] rather
/// than an error, which is what the consent screen did with `.ok().flatten()`: a
/// guild that could not be read and one that is not there are the same nothing to
/// act for.
pub async fn guild_access(state: &AppState, guild_id: i64, account_id: i32) -> GuildAccess {
    // Ownership is authority, so never use the display cache here.
    let Ok((200, guild)) = state.discord().get_guild_with_status(guild_id).await else {
        return GuildAccess::Unknown;
    };

    // A grant belongs to a guild, so the bot has to be in it: one the bot cannot
    // see is not a guild it can grant anything in.
    if state
        .discord()
        .get_guild_member(guild_id, state.discord().bot_user_id())
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return GuildAccess::Unknown;
    }

    // The Elixir used the session's VirtualCrypto id as a Discord id here, which is
    // why its answer was always no.
    let Some(discord_id) = vc_core::user::find_by_id(state.pool(), account_id)
        .await
        .ok()
        .flatten()
        .and_then(|account| account.discord_id)
    else {
        return GuildAccess::Denied;
    };

    let Some(member) = state
        .discord()
        .get_guild_member(guild_id, discord_id)
        .await
        .ok()
        .flatten()
    else {
        return GuildAccess::Unknown;
    };

    let roles = state
        .discord()
        .get_roles(guild_id)
        .await
        .unwrap_or_default();

    let Some(facts) = guild_facts(&guild, &member, &roles) else {
        return GuildAccess::Unknown;
    };

    if may_act_for_guild(discord_id, &facts) {
        GuildAccess::Permitted
    } else {
        GuildAccess::Denied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_member_holds_what_their_roles_hold() {
        assert_eq!(member_permissions(&[7], &[(7, 0x100)]), 0x100);
    }

    /// The OR is over the roles, not over the member: a bit any one role carries
    /// is a bit the member has.
    #[test]
    fn roles_are_ored_together() {
        assert_eq!(
            member_permissions(&[7, 8], &[(7, 0x100), (8, 0x200)]),
            0x300
        );
    }

    #[test]
    fn a_role_the_member_does_not_have_does_not_count() {
        assert_eq!(member_permissions(&[7], &[(7, 0x100), (8, 0x200)]), 0x100);
    }

    /// The case the Elixir would raise on. Discord's own lists agree, so this is
    /// about not turning a surprise into an outage.
    #[test]
    fn a_role_the_guild_does_not_have_is_skipped() {
        assert_eq!(member_permissions(&[7, 999], &[(7, 0x100)]), 0x100);
    }

    #[test]
    fn a_member_with_no_roles_holds_nothing() {
        assert_eq!(member_permissions(&[], &[(7, 0x100)]), 0);
    }

    /// How the consent screen will use it: the mask, then the one question the
    /// commands already ask.
    #[test]
    fn the_administrator_bit_survives_a_role_that_carries_it() {
        assert!(is_administrator(member_permissions(
            &[7, 8],
            &[(7, 0x1), (8, 0x8)]
        )));
        assert!(!is_administrator(member_permissions(&[7], &[(7, 0x1)])));
    }

    fn facts() -> GuildFacts {
        guild_facts(
            &json!({ "owner_id": "10" }).as_object().cloned().unwrap(),
            &json!({ "roles": ["20", "21"] })
                .as_object()
                .cloned()
                .unwrap(),
            &[
                json!({ "id": "20", "permissions": "1" })
                    .as_object()
                    .cloned()
                    .unwrap(),
                json!({ "id": "21", "permissions": "8" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ],
        )
        .expect("facts")
    }

    /// All three numbers arrive as strings, which is the whole reason this is a
    /// function rather than three lines in a handler.
    #[test]
    fn the_numbers_are_read_out_of_strings() {
        assert_eq!(
            facts(),
            GuildFacts {
                owner_id: 10,
                member_role_ids: vec![20, 21],
                roles: vec![(20, 1), (21, 8)],
            }
        );
    }

    #[test]
    fn the_owner_may_act_without_any_role() {
        let facts = GuildFacts {
            owner_id: 10,
            member_role_ids: vec![],
            roles: vec![],
        };

        assert!(may_act_for_guild(10, &facts));
    }

    #[test]
    fn an_administrator_role_may_act() {
        assert!(may_act_for_guild(99, &facts()));
    }

    /// A member who is neither the owner nor an administrator may not, which is
    /// the answer the Elixir gave to everyone.
    #[test]
    fn a_member_without_the_bit_may_not() {
        let facts = GuildFacts {
            owner_id: 10,
            member_role_ids: vec![20],
            roles: vec![(20, 1)],
        };

        assert!(!may_act_for_guild(99, &facts));
    }

    #[test]
    fn an_unreadable_payload_is_no_facts() {
        assert_eq!(
            guild_facts(
                &json!({}).as_object().cloned().unwrap(),
                &json!({ "roles": [] }).as_object().cloned().unwrap(),
                &[],
            ),
            None
        );

        assert_eq!(
            guild_facts(
                &json!({ "owner_id": "10" }).as_object().cloned().unwrap(),
                &json!({ "roles": ["20"] }).as_object().cloned().unwrap(),
                &[json!({ "id": "20" }).as_object().cloned().unwrap()],
            ),
            None
        );
    }
}
