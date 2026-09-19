//! The day a job last ran, which is the one thing about a job the domain cannot
//! say.
//!
//! A tick is a minute and a job may be daily: `reset_pool_amount` adds a day's
//! allowance with no memory of the last one, so something has to hold the day.
//! A variable holds it until the process restarts, and a restart would buy
//! another day's allowance — so the day is a row.

use sqlx::PgPool;
use time::Date;

/// Claim `name`'s day: `true` when the job has not run on `today` yet.
///
/// The claim is the write itself, so two schedulers that tick in the same second
/// — two machines behind one database — cannot both take the day: only the one
/// that finds an earlier day updates a row, and only an updated row is returned.
/// A claim that is taken and then fails loses that day rather than paying it
/// twice, which is the side to lose on when paying is inflation.
pub async fn claim_day(pool: &PgPool, name: &str, today: Date) -> Result<bool, sqlx::Error> {
    let claimed = sqlx::query_scalar!(
        r#"
        INSERT INTO job_runs (name, ran_on) VALUES ($1, $2)
        ON CONFLICT (name) DO UPDATE SET ran_on = EXCLUDED.ran_on
        WHERE job_runs.ran_on < EXCLUDED.ran_on
        RETURNING name AS "name!"
        "#,
        name,
        today
    )
    .fetch_optional(pool)
    .await?;

    Ok(claimed.is_some())
}
