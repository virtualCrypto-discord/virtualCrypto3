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
