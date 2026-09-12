use time::{Duration, PrimitiveDateTime};

/// How long a stored Discord authorization is considered valid.
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
    pub updated_at: PrimitiveDateTime,
}

impl DiscordAuth {
    /// Mirrors `VirtualCrypto.DiscordAuth.refresh_user/1`: refresh when the
    /// stored authorization is within 15 minutes of its seven-day lifetime.
    /// The difference is negative once it is overdue, which also refreshes.
    pub fn needs_refresh(&self, now: PrimitiveDateTime) -> bool {
        let expires_at = self.updated_at + DISCORD_AUTH_LIFETIME;
        (expires_at - now) <= DISCORD_REFRESH_MARGIN
    }
}
