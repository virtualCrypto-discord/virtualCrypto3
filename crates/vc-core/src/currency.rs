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
///   `Decimal.to_integer/1`, so the sum is cast back to `bigint` here. That is the
///   amount issued so far, because nothing but deleting the currency removes an
///   `assets` row: a payment moves one, a contract holds one in the contract's own
///   account, and the pool is not in `assets` at all;
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
/// A concurrent insert can pass the same checks. Its uniqueness violation is
/// mapped to the corresponding duplicate error; other database failures remain
/// database errors.
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
    .map_err(|error| {
        if let sqlx::Error::Database(ref database) = error
            && database.is_unique_violation()
        {
            match database.constraint() {
                Some("info_guild_id_index") => return CreateError::Guild,
                Some("info_unit_index") => return CreateError::Unit,
                Some("info_name_index") => return CreateError::Name,
                _ => {}
            }
        }
        CreateError::Database(error)
    })?;

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

/// `Query.Currency.reset_pool_amount/0`: every pool gets a day's allowance.
///
/// The rule as published is 0.5% of the total issuance a day, up to 3.5% of it.
/// What the Elixir's SQL actually measures is
/// **the supply in users' hands** — `SUM(assets.amount)` per currency — and not
/// the creator's initial grant, so a currency whose users hold more gets more; a
/// currency nobody holds is not in the join at all and is left alone. The
/// allowance is `(supplied + 199) / 200` and the ceiling `(supplied * 7 + 199) /
/// 200`, both floored at 5 and 35; the division is numeric, as Postgres reads a
/// `SUM` of `bigint`, so the value is rounded on the way into the column rather
/// than truncated — which is the Elixir's arithmetic and not the ceiling its
/// `+199` suggests.
///
/// One statement over every currency, and an increment rather than a target: it
/// is run once a day, so running it twice is running two days' worth. That is
/// what the schedule is for, and it is why this takes no `now` — nothing in the
/// data says when it last ran.
pub async fn reset_pool_amount(
    connection: &mut sqlx::PgConnection,
) -> std::result::Result<u64, sqlx::Error> {
    let updated = sqlx::query!(
        r#"WITH supplied_amounts AS (
             SELECT currency_id, SUM(amount) AS supplied_amount
               FROM assets GROUP BY currency_id
           ), schedules AS (
             SELECT currency_id,
                    CASE WHEN (supplied_amounts.supplied_amount + 199) / 200 < 5
                         THEN 5
                         ELSE (supplied_amounts.supplied_amount + 199) / 200
                    END AS increasing_pool_amount,
                    CASE WHEN (supplied_amounts.supplied_amount * 7 + 199) / 200 < 35
                         THEN 35
                         ELSE (supplied_amounts.supplied_amount * 7 + 199) / 200
                    END AS pool_amount_limit
               FROM supplied_amounts
           )
           UPDATE currencies
              SET pool_amount = CASE
                  WHEN schedules.increasing_pool_amount + currencies.pool_amount
                       > schedules.pool_amount_limit
                  THEN schedules.pool_amount_limit
                  ELSE schedules.increasing_pool_amount + currencies.pool_amount
              END
             FROM schedules
            WHERE schedules.currency_id = currencies.id"#
    )
    .execute(connection)
    .await?;

    Ok(updated.rows_affected())
}

/// The guild's currency unit, for the confirmation the delete modal asks for.
pub async fn unit_for_guild(
    pool: &PgPool,
    guild_id: i64,
) -> std::result::Result<Option<String>, sqlx::Error> {
    let unit = sqlx::query_scalar!("SELECT unit FROM currencies WHERE guild_id = $1", guild_id)
        .fetch_optional(pool)
        .await?;

    Ok(unit.flatten())
}

/// `Query.Currency.delete/1`: the currency and everything that hangs off it.
///
/// `claim_metadata` is not named because its foreign key to `claims` cascades,
/// which is also why Elixir can leave it out.
pub async fn delete(
    pool: &PgPool,
    guild_id: i64,
    confirmation: &str,
) -> std::result::Result<DeleteResult, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let currency = sqlx::query!(
        "SELECT id, unit, inserted_at FROM currencies WHERE guild_id = $1 FOR UPDATE",
        guild_id
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(currency) = currency else {
        return Ok(DeleteResult::NotExist);
    };

    // Check after acquiring the lock: a modal can outlive the deletion window,
    // and confirmation must apply to the same currency we actually delete.
    if (crate::model::utc_now() - currency.inserted_at).whole_seconds() > DELETABLE_WINDOW {
        return Ok(DeleteResult::OutOfTerm);
    }
    let required = format!("delete {}", currency.unit.unwrap_or_default());
    if confirmation.to_lowercase() != required.to_lowercase() {
        return Ok(DeleteResult::ConfirmationFailed);
    }
    let currency_id = currency.id;

    for statement in [
        "DELETE FROM assets WHERE currency_id = $1",
        "DELETE FROM currency_given_histories WHERE currency_id = $1",
        "DELETE FROM currency_payment_histories WHERE currency_id = $1",
        "DELETE FROM claims WHERE currency_id = $1",
        // Approval creates an escrow user that still references the contract
        // after withdrawal/refund. Remove it after its assets, before contracts.
        "DELETE FROM users WHERE contract_id IN (SELECT id FROM contracts WHERE currency_id = $1)",
        // The parties go with the contract's row, which cascades: a currency
        // that is deleted is one whose locked money is deleted with it.
        "DELETE FROM contracts WHERE currency_id = $1",
    ] {
        sqlx::query(statement)
            .bind(currency_id)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query!("DELETE FROM currencies WHERE id = $1", currency_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(DeleteResult::Deleted)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteResult {
    Deleted,
    NotExist,
    OutOfTerm,
    ConfirmationFailed,
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

/// One option candidate: a currency, and what the caller holds of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrencyCandidate {
    pub amount: i64,
    pub name: Option<String>,
    pub unit: Option<String>,
}

/// `escape_like_query/1`: a prefix match, with the wildcards in the user's own
/// text escaped so a `%` they typed does not match everything.
fn like_prefix(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 1);

    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }

    escaped.push('%');

    escaped
}

/// `Money.search_currencies_with_asset_by_unit/3`: the units starting with
/// `unit`, the caller's own guild first and their holdings next — the order the
/// command's suggestions appear in. Muted currencies are excluded before the
/// limit.
pub async fn search_by_unit(
    pool: &PgPool,
    unit: &str,
    guild_id: Option<i64>,
    operator_id: i32,
    limit: i64,
) -> Result<Vec<CurrencyCandidate>> {
    let candidates = sqlx::query_as!(
        CurrencyCandidate,
        "SELECT COALESCE(assets.amount, 0) AS \"amount!\",
                currencies.name, currencies.unit
           FROM currencies
           LEFT JOIN assets
                  ON assets.currency_id = currencies.id AND assets.user_id = $1
          WHERE currencies.unit ILIKE $2
            AND NOT EXISTS (
                SELECT 1 FROM mutes
                 WHERE mutes.user_id = $1 AND mutes.currency_id = currencies.id
            )
          ORDER BY (currencies.guild_id = $3) DESC NULLS LAST,
                   (COALESCE(assets.amount, 0) != 0) DESC,
                   char_length(currencies.unit) ASC,
                   assets.updated_at DESC NULLS LAST
          LIMIT $4",
        i64::from(operator_id),
        like_prefix(unit),
        guild_id,
        limit
    )
    .fetch_all(pool)
    .await?;

    Ok(candidates)
}

/// `Money.search_currencies_with_asset_by_name/3`, which orders by the name's
/// length and then by the currency's id rather than by anything stored on the
/// asset. Mute filtering follows [`search_by_unit`].
pub async fn search_by_name(
    pool: &PgPool,
    name: &str,
    guild_id: Option<i64>,
    operator_id: i32,
    limit: i64,
) -> Result<Vec<CurrencyCandidate>> {
    let candidates = sqlx::query_as!(
        CurrencyCandidate,
        "SELECT COALESCE(assets.amount, 0) AS \"amount!\",
                currencies.name, currencies.unit
           FROM currencies
           LEFT JOIN assets
                  ON assets.currency_id = currencies.id AND assets.user_id = $1
          WHERE currencies.name ILIKE $2
            AND NOT EXISTS (
                SELECT 1 FROM mutes
                 WHERE mutes.user_id = $1 AND mutes.currency_id = currencies.id
            )
          ORDER BY (currencies.guild_id = $3) DESC NULLS LAST,
                   (COALESCE(assets.amount, 0) != 0) DESC,
                   char_length(currencies.name) ASC,
                   currencies.id ASC
          LIMIT $4",
        i64::from(operator_id),
        like_prefix(name),
        guild_id,
        limit
    )
    .fetch_all(pool)
    .await?;

    Ok(candidates)
}

/// `Money.search_currencies_with_asset_by_guild_and_user/2`, which is what an
/// empty query offers: the caller's own currencies and their guild's.
/// Mute filtering follows [`search_by_unit`], including the guild's currency.
pub async fn search_by_guild_and_user(
    pool: &PgPool,
    guild_id: Option<i64>,
    operator_id: i32,
    limit: i64,
) -> Result<Vec<CurrencyCandidate>> {
    let candidates = sqlx::query_as!(
        CurrencyCandidate,
        "SELECT COALESCE(assets.amount, 0) AS \"amount!\",
                currencies.name, currencies.unit
           FROM currencies
           LEFT JOIN assets
                  ON assets.currency_id = currencies.id AND assets.user_id = $1
          WHERE (assets.user_id = $1 OR currencies.guild_id = $2)
            AND NOT EXISTS (
                SELECT 1 FROM mutes
                 WHERE mutes.user_id = $1 AND mutes.currency_id = currencies.id
            )
          ORDER BY (currencies.guild_id = $2) DESC NULLS LAST,
                   (COALESCE(assets.amount, 0) != 0) DESC,
                   currencies.id ASC
          LIMIT $3",
        i64::from(operator_id),
        guild_id,
        limit
    )
    .fetch_all(pool)
    .await?;

    Ok(candidates)
}
