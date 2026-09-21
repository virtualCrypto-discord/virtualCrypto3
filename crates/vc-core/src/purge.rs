//! Deleting the rows whose time is up.
//!
//! Five tables carry an `expires` and nothing in this service acted on it. The
//! Elixir removes the expired ones on a timer — `tests/golden/README.md` records
//! it for `user_access_tokens` — so a deployment of this service accumulated rows
//! nobody could use again, and for `payments_idempotency` that is not only
//! storage: a key whose week is up is meant to stop replaying an answer, and the
//! row is what says the week is up.
//!
//! What a row is, per table: a signed token (`user_access_tokens`), an
//! idempotency key (`payments_idempotency`), an authorization code nobody
//! redeemed (`authorization_codes`), a grant's access token and its refresh token
//! (`access_tokens`, `refresh_tokens`), and an ask a device made
//! (`grant_requests`). Every one is meaningless once its expiry has passed — the
//! reads refuse them by comparison — so deleting them is what keeps the tables the
//! size of what is still usable. An ask is the plainest of the six: its
//! `device_code`, its `user_code` and the scopes it asked for are read only while
//! the ask is alive, and past `inserted_at + expires_in` the device is refused,
//! the guild's approval is refused, and nothing is left to read them for.
//!
//! The Elixir's cron names three of those jobs: the signed tokens, the access
//! tokens, and the idempotency keys. The codes, the refresh tokens and the asks go
//! with them, which is an addition: they carry the same lifetime, nothing refuses
//! them by anything else, and left alone they are rows that only ever grow.

use sqlx::PgPool;
use time::PrimitiveDateTime;

/// One statement per table rather than one per row, and a single call, so the
/// cost is the same whether nothing has expired or everything has.
///
/// The ask is the one whose expiry is not a column: it is the moment it was made
/// plus the `expires_in` it asked for, which is the same sum its two readers
/// compute.
const EXPIRING: [&str; 6] = [
    "DELETE FROM user_access_tokens WHERE expires < $1",
    "DELETE FROM payments_idempotency WHERE expires < $1",
    "DELETE FROM authorization_codes WHERE expires < $1",
    "DELETE FROM access_tokens WHERE expires < $1",
    "DELETE FROM refresh_tokens WHERE expires < $1",
    "DELETE FROM grant_requests WHERE inserted_at + make_interval(secs => expires_in) < $1",
];

/// How many rows went, which is what a caller logs: zero is the common answer and
/// is not worth a line.
pub async fn expired(
    pool: &PgPool,
    now: PrimitiveDateTime,
) -> std::result::Result<u64, sqlx::Error> {
    let mut deleted = 0;

    for statement in EXPIRING {
        deleted += sqlx::query(statement)
            .bind(now)
            .execute(pool)
            .await?
            .rows_affected();
    }

    Ok(deleted)
}
