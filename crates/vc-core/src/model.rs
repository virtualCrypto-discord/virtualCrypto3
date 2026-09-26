use time::{Duration, PrimitiveDateTime};

/// Legacy lifetime used only when a Discord authorization has no stored expiry.
pub const DISCORD_AUTH_LIFETIME: Duration = Duration::days(7);
/// Refresh once the authorization is this close to expiring.
pub const DISCORD_REFRESH_MARGIN: Duration = Duration::minutes(15);

/// Current UTC time as the naive, second-precision timestamp the schema stores.
/// Ecto truncates to whole seconds on every write, so do the same here.
pub fn utc_now() -> PrimitiveDateTime {
    let now = time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero is a valid nanosecond");
    PrimitiveDateTime::new(now.date(), now.time())
}

/// A virtualCrypto account. `users.id` is the only `integer` primary key in the
/// schema; every other id column is `bigint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i32,
    pub discord_id: Option<i64>,
    pub status: Option<i32>,
}

/// The stored Discord OAuth2 authorization for a discord user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscordAuth {
    pub discord_user_id: i64,
    pub token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires: Option<PrimitiveDateTime>,
    pub updated_at: PrimitiveDateTime,
}

impl DiscordAuth {
    /// Refresh within 15 minutes of the expiry Discord supplied, or once overdue.
    /// Rows without an expiry retain the Elixir's seven-day lifetime estimate.
    pub fn needs_refresh(&self, now: PrimitiveDateTime) -> bool {
        let expires_at = self
            .expires
            .unwrap_or_else(|| self.updated_at + DISCORD_AUTH_LIFETIME);
        (expires_at - now) <= DISCORD_REFRESH_MARGIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discord_refresh_respects_the_expiry_margin_and_legacy_fallback() {
        let now = utc_now();
        let margin = Duration::minutes(15);
        let second = Duration::seconds(1);
        let week = Duration::days(7);
        for (expires_in, updated_ago, expected) in [
            (Some(margin + second), week, false),
            (Some(margin), Duration::ZERO, true),
            (Some(Duration::ZERO), Duration::ZERO, true),
            (Some(-second), Duration::ZERO, true),
            (None, Duration::ZERO, false),
            (None, week - margin - second, false),
            (None, week - margin, true),
            (None, week + second, true),
        ] {
            let authorization = DiscordAuth {
                discord_user_id: 1,
                token: None,
                refresh_token: None,
                expires: expires_in.map(|remaining| now + remaining),
                updated_at: now - updated_ago,
            };
            assert_eq!(
                authorization.needs_refresh(now),
                expected,
                "expires_in={expires_in:?}, updated_ago={updated_ago:?}"
            );
        }
    }
}
