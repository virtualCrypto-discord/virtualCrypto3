use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};

/// Migrations embedded at compile time from `crates/vc-core/migrations`.
///
/// The baseline migration is idempotent, so this applies cleanly both to a fresh
/// database and to the existing production database (see scripts/baseline-check.sh).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

pub async fn connect(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await
}

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}
