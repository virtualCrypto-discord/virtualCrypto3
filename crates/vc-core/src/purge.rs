//! Deleting the rows whose time is up.
//!
//! Three tables carry an `expires` and nothing in this service acted on it. The
//! Elixir removes the expired ones on a timer — `tests/golden/README.md` records
//! it for `user_access_tokens` — so a deployment of this service accumulated rows
//! nobody could use again, and for `payments_idempotency` that is not only
//! storage: a key whose week is up is meant to stop replaying an answer, and the
//! row is what says the week is up.
//!
//! What a row is, per table: a signed token (`user_access_tokens`), an
//! idempotency key (`payments_idempotency`), or an authorization code nobody
//! redeemed (`authorization_codes`). All three are meaningless once `expires` has
//! passed — the verifiers refuse them by comparison — so deleting them is what
//! keeps the tables the size of what is still usable.

use sqlx::PgPool;
use time::PrimitiveDateTime;

/// One statement per table rather than one per row, and a single call, so the
/// cost is the same whether nothing has expired or everything has.
const EXPIRING: [&str; 3] = [
    "DELETE FROM user_access_tokens WHERE expires < $1",
    "DELETE FROM payments_idempotency WHERE expires < $1",
    "DELETE FROM authorization_codes WHERE expires < $1",
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
