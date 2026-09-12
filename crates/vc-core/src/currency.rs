use sqlx::PgPool;
use time::PrimitiveDateTime;

use crate::error::Result;

/// `Money.is_deletable?/2`: a currency may be deleted for three days after it
/// was created.
pub const DELETABLE_WINDOW: i64 = 3 * 24 * 60 * 60;

/// How `Money.info/1` is told which currency to look up. The four variants mirror
/// the four `Query.Currency.info/2` clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrencySelector<'a> {
    Id(i64),
    Guild(i64),
    Name(&'a str),
    Unit(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrencyInfo {
    pub total_amount: i64,
    pub name: Option<String>,
    pub unit: Option<String>,
    pub guild_id: Option<i64>,
    pub pool_amount: Option<i64>,
    pub inserted_at: PrimitiveDateTime,
}

impl CurrencyInfo {
    /// `Money.is_deletable?/2`.
    pub fn is_deletable(&self, now: PrimitiveDateTime) -> bool {
        (now - self.inserted_at).whole_seconds() <= DELETABLE_WINDOW
    }
}

/// Mirrors `Money.info/1` backed by `Query.Currency.info/2`.
///
/// Two behaviours are worth keeping in mind:
///
/// * the total is `sum(assets.amount)` — Ecto reads it as `numeric` and calls
///   `Decimal.to_integer/1`, so the sum is cast back to `bigint` here;
/// * the query joins `assets`, so a currency that has no asset rows produces no
///   row at all and is reported as not found.
pub async fn info(pool: &PgPool, selector: CurrencySelector<'_>) -> Result<Option<CurrencyInfo>> {
    let row = match selector {
        CurrencySelector::Id(id) => {
            sqlx::query_as!(
                CurrencyInfo,
                "SELECT sum(assets.amount)::bigint AS \"total_amount!\",
                        currencies.name, currencies.unit, currencies.guild_id, currencies.pool_amount,
                        currencies.inserted_at
                   FROM assets JOIN currencies ON assets.currency_id = currencies.id
                  WHERE currencies.id = $1
                  GROUP BY currencies.id",
                id
            )
            .fetch_optional(pool)
            .await?
        }
        CurrencySelector::Guild(guild_id) => {
            sqlx::query_as!(
                CurrencyInfo,
                "SELECT sum(assets.amount)::bigint AS \"total_amount!\",
                        currencies.name, currencies.unit, currencies.guild_id, currencies.pool_amount,
                        currencies.inserted_at
                   FROM assets JOIN currencies ON assets.currency_id = currencies.id
                  WHERE currencies.guild_id = $1
                  GROUP BY currencies.id",
                guild_id
            )
            .fetch_optional(pool)
            .await?
        }
        CurrencySelector::Name(name) => {
            sqlx::query_as!(
                CurrencyInfo,
                "SELECT sum(assets.amount)::bigint AS \"total_amount!\",
                        currencies.name, currencies.unit, currencies.guild_id, currencies.pool_amount,
                        currencies.inserted_at
                   FROM assets JOIN currencies ON assets.currency_id = currencies.id
                  WHERE currencies.name = $1
                  GROUP BY currencies.id",
                name
            )
            .fetch_optional(pool)
            .await?
        }
        CurrencySelector::Unit(unit) => {
            sqlx::query_as!(
                CurrencyInfo,
                "SELECT sum(assets.amount)::bigint AS \"total_amount!\",
                        currencies.name, currencies.unit, currencies.guild_id, currencies.pool_amount,
                        currencies.inserted_at
                   FROM assets JOIN currencies ON assets.currency_id = currencies.id
                  WHERE currencies.unit = $1
                  GROUP BY currencies.id",
                unit
            )
            .fetch_optional(pool)
            .await?
        }
    };

    Ok(row)
}
