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

/// One page of a discord user's holdings, and where the pages around it are — the shape a
/// screen that shows a page and four arrows needs, as `OpenContracts` is for contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Balances {
    pub balances: Vec<Balance>,
    /// How many there are altogether, which is the number the screen may say.
    pub total: i64,
    /// Which page this is, counting from one, as the claim list's does.
    pub page: i64,
    /// The four places the arrows move to, each `None` when there is nowhere to go — which
    /// is what makes a button disabled rather than absent.
    pub first: Option<i64>,
    pub prev: Option<i64>,
    pub next: Option<i64>,
    pub last: Option<i64>,
}

/// One page of what a discord user holds, in unit order, and the count behind it.
///
/// The join on `users` is what makes this take a discord id, and it also means a discord id
/// with no account simply has no rows — nothing is created here, and an empty page is a page
/// with nothing on it rather than an error.
pub async fn page_for_discord_user(
    pool: &PgPool,
    discord_user_id: i64,
    page: i64,
    limit: i64,
) -> Result<Balances> {
    let page = page.max(1);

    let total = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\"
           FROM assets
           JOIN users ON users.id = assets.user_id
          WHERE users.discord_id = $1",
        discord_user_id
    )
    .fetch_one(pool)
    .await?;

    let rows = sqlx::query!(
        "SELECT assets.amount, currencies.name, currencies.unit
           FROM assets
           JOIN currencies ON currencies.id = assets.currency_id
           JOIN users ON users.id = assets.user_id
          WHERE users.discord_id = $1
          ORDER BY currencies.unit
          LIMIT $2
         OFFSET $3",
        discord_user_id,
        limit,
        limit * (page - 1)
    )
    .fetch_all(pool)
    .await?;

    let last_page = ((total + limit - 1) / limit).max(1);

    Ok(Balances {
        balances: rows
            .into_iter()
            .map(|row| Balance {
                amount: row.amount.unwrap_or(0),
                name: row.name.unwrap_or_default(),
                unit: row.unit.unwrap_or_default(),
            })
            .collect(),
        total,
        page,
        first: (page > 1).then_some(1),
        prev: (page > 1).then_some(page - 1),
        next: (page < last_page).then_some(page + 1),
        last: (page < last_page).then_some(last_page),
    })
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
