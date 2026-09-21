use sqlx::PgPool;

use crate::error::Result;

/// One row of `Query.Balance.get_balances/1`: the asset and the currency it is
/// denominated in, flattened to the three fields the renderers read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Balance {
    pub amount: i64,
    pub name: String,
    pub unit: String,
}

/// `Query.Balance.get_balances/1` for a discord user: their assets joined to the
/// currencies, ordered by unit.
///
/// The join on `users` is what makes this take a discord id, and it also means a
/// discord id with no account simply has no rows — nothing is created here.
pub async fn for_discord_user(pool: &PgPool, discord_user_id: i64) -> Result<Vec<Balance>> {
    let rows = sqlx::query!(
        "SELECT assets.amount, currencies.name, currencies.unit
           FROM assets
           JOIN currencies ON currencies.id = assets.currency_id
           JOIN users ON users.id = assets.user_id
          WHERE users.discord_id = $1
          ORDER BY currencies.unit",
        discord_user_id
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| Balance {
            amount: row.amount.unwrap_or(0),
            name: row.name.unwrap_or_default(),
            unit: row.unit.unwrap_or_default(),
        })
        .collect())
}

/// An account's holdings as the API answers them: the amount, and the currency.
///
/// Wider than [`Balance`] by two fields — the guild the currency belongs to and
/// its pool — because the API shows them and the Discord message does not.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Holding {
    /// Nullable in the schema, like the two below it.
    pub amount: Option<i64>,
    pub guild: Option<i64>,
    pub name: Option<String>,
    pub pool_amount: Option<i64>,
    pub unit: Option<String>,
}

/// `GET /api/v2/users/@me/balances`: holdings for the authenticated internal
/// account, including applications without a linked Discord user.
pub async fn holdings_for_user(pool: &PgPool, user_id: i32) -> Result<Vec<Holding>> {
    let rows = sqlx::query_as!(
        Holding,
        "SELECT assets.amount,
                currencies.guild_id AS guild,
                currencies.name,
                currencies.pool_amount,
                currencies.unit
           FROM assets
           JOIN currencies ON currencies.id = assets.currency_id
          WHERE assets.user_id = $1
          ORDER BY currencies.unit",
        i64::from(user_id)
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
