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

/// `Money.create/1`'s guard on the creator's grant.
pub const MAX_CREATOR_AMOUNT: i64 = 4_294_967_295;

#[derive(Debug)]
pub enum CreateError {
    /// The guild already has a currency.
    Guild,
    /// The unit is taken, in any guild.
    Unit,
    /// The name is taken, in any guild.
    Name,
    InvalidAmount,
    Database(sqlx::Error),
}

/// `Money.create/1` through `Query.Currency.create/6`.
///
/// The pool is a two-hundredth of the creator's grant, rounded up, with a floor
/// of five. The three uniqueness checks run in the order the Elixir `with` runs
/// them — guild, then unit, then name — so the first conflict decides the error,
/// and the creator is resolved (creating their account if needed) only once all
/// three have passed.
///
/// Elixir wraps the insert in a retry loop, which covers a concurrent insert
/// slipping past those checks. That is not reproduced, so such a race would
/// surface as a database error instead.
pub async fn create(
    pool: &PgPool,
    guild_id: i64,
    name: &str,
    unit: &str,
    creator_discord_id: i64,
    creator_amount: i64,
) -> std::result::Result<(), CreateError> {
    if !(0..=MAX_CREATOR_AMOUNT).contains(&creator_amount) {
        return Err(CreateError::InvalidAmount);
    }

    let pool_amount = ((creator_amount + 199) / 200).max(5);
    let mut tx = pool.begin().await.map_err(CreateError::Database)?;

    let taken = sqlx::query_scalar!("SELECT id FROM currencies WHERE guild_id = $1", guild_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CreateError::Database)?;
    if taken.is_some() {
        return Err(CreateError::Guild);
    }

    let taken = sqlx::query_scalar!("SELECT id FROM currencies WHERE unit = $1", unit)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CreateError::Database)?;
    if taken.is_some() {
        return Err(CreateError::Unit);
    }

    let taken = sqlx::query_scalar!("SELECT id FROM currencies WHERE name = $1", name)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CreateError::Database)?;
    if taken.is_some() {
        return Err(CreateError::Name);
    }

    let creator = crate::user::insert_if_not_exists(&mut tx, creator_discord_id)
        .await
        .map_err(CreateError::Database)?;

    let now = crate::model::utc_now();

    let currency_id = sqlx::query_scalar!(
        "INSERT INTO currencies (guild_id, pool_amount, name, unit, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $5)
         RETURNING id",
        guild_id,
        pool_amount,
        name,
        unit,
        now
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(CreateError::Database)?;

    sqlx::query!(
        "INSERT INTO assets (amount, user_id, currency_id, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)",
        creator_amount,
        i64::from(creator.id),
        currency_id,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(CreateError::Database)?;

    tx.commit().await.map_err(CreateError::Database)?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteCheck {
    /// The currency is inside its deletion window.
    Deletable { unit: String },
    /// The guild has no currency at all.
    NotExist,
    /// The window has closed.
    OutOfTerm,
}

/// `Money.delete/2` with `dry_run: true`, which is what `/delete` asks for: the
/// caller only needs to know whether to show the confirmation modal.
pub async fn deletable(
    pool: &PgPool,
    guild_id: i64,
    now: PrimitiveDateTime,
) -> std::result::Result<DeleteCheck, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT unit, inserted_at FROM currencies WHERE guild_id = $1 FOR UPDATE",
        guild_id
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(DeleteCheck::NotExist);
    };

    if (now - row.inserted_at).whole_seconds() <= DELETABLE_WINDOW {
        Ok(DeleteCheck::Deletable {
            unit: row.unit.unwrap_or_default(),
        })
    } else {
        Ok(DeleteCheck::OutOfTerm)
    }
}
