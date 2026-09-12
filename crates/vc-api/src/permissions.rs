//! Discord permissions, as far as this service asks about them.
//!
//! An interaction arrives with the caller's permissions already computed by
//! Discord, which is why the commands need only
//! [`crate::command::is_administrator`] applied to a number. A consent screen is
//! a REST lookup instead, so the number has to be built here — the Elixir ORs
//! together the permissions of the roles the member carries.

/// The permissions a member has: the permissions of their roles, ORed together.
///
/// A role id the guild does not have is skipped. The Elixir instead indexes the
/// guild's roles with each of the member's ids and calls `String.to_integer/1` on
/// whatever comes back, so an unknown id is a crash there rather than a skip. The
/// two lists agree in any guild Discord has not mangled, which is why this is a
/// note rather than a decision — but a crash is not worth reproducing for a case
/// that cannot be reached.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::is_administrator;

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
}
