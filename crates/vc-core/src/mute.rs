//! What a person has chosen not to see: a currency, or somebody whose actions would otherwise
//! appear in their lists.
//!
//! A mute is a filter and not a lock. Nothing here refuses a payment, blocks a claim or stops an
//! issue: balances, histories, claims, contracts and their autocomplete leave out muted rows
//! for the person who set it,
//! and every other reader of that data — another person, an application, a single row fetched by
//! id — sees what they always saw. That is why the pair a row holds is (who is looking, what
//! they are not looking at) and why a mute cannot be aimed at somebody: being named in a row
//! never changes what its target sees.
//!
//! 無期限: there is no column for an end, because a preference has no sentence to serve — what
//! ends one is [`unmute_currency_id`] or [`unmute_user`], which is the same `/mute list` button the
//! person who set it pressed.
//!
//! An addition rather than a port: the Elixir has no mute and nothing that filters a list by its
//! reader, so this is this service's own shape — a table the Elixir neither reads nor writes.

use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;

/// How many mutes one screen shows. The claim list's own arithmetic: five rows, four arrows, and
/// a page small enough that a person reads the line they mean to press.
pub const PER_PAGE: i64 = 5;

/// What a person is not seeing, as a screen has to say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A currency: the id a button carries, and what a line names it by.
    Currency { id: i64, unit: String, name: String },
    /// A person: the Discord id every screen mentions them by.
    User { discord_id: i64 },
}

/// One page of what a person is not seeing, and the count behind it.
///
/// The same shape the claim and contract lists answer with, so the arrows that move it are the
/// same arithmetic in the same places.
#[derive(Debug, Clone, PartialEq)]
pub struct Mutes {
    pub mutes: Vec<Target>,
    /// How many there are altogether, which is the number the screen may say.
    pub total: i64,
    /// Which page this is, counting from one.
    pub page: i64,
    /// The four places the arrows move to, each `None` when there is nowhere to go — which is
    /// what makes a button disabled rather than absent.
    pub first: Option<i64>,
    pub prev: Option<i64>,
    pub next: Option<i64>,
    pub last: Option<i64>,
}

/// Why a mute was not set.
#[derive(Debug)]
pub enum MuteError {
    /// No currency has that unit: there is nothing to leave out of a list.
    NoSuchCurrency,
    /// No account has that Discord id. A person who has never used this service has no actions
    /// here to hide, and a mute naming them would be a row about nobody.
    NoSuchUser,
    /// Your own actions are what a list is about: muting yourself would empty it of you rather
    /// than of anybody else.
    Yourself,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for MuteError {
    fn from(error: sqlx::Error) -> Self {
        MuteError::Database(error)
    }
}

/// Leaving a currency out of the caller's lists. `true` when it was not already muted, which is
/// what tells 「ミュートしました」 from 「すでにミュートしています」.
pub async fn mute_currency(
    pool: &PgPool,
    user_id: i32,
    unit: &str,
    now: OffsetDateTime,
) -> Result<bool, MuteError> {
    let mut tx = pool.begin().await?;
    // Keep the currency alive through insertion, before the mute's foreign-key
    // checks acquire account locks. Deletion takes the currency lock first too.
    let Some(currency_id) = sqlx::query_scalar!(
        "SELECT id FROM currencies WHERE unit = $1 FOR KEY SHARE",
        unit
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(MuteError::NoSuchCurrency);
    };

    let inserted = sqlx::query!(
        "INSERT INTO mutes (user_id, currency_id, inserted_at)
              VALUES ($1, $2, $3)
         ON CONFLICT (user_id, currency_id) WHERE currency_id IS NOT NULL DO NOTHING",
        user_id,
        currency_id,
        now
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    tx.commit().await?;
    Ok(inserted == 1)
}

/// Leaving somebody out of the caller's lists. `true` when it was not already muted.
pub async fn mute_user(
    pool: &PgPool,
    user_id: i32,
    discord_id: i64,
    now: OffsetDateTime,
) -> Result<bool, MuteError> {
    let mut tx = pool.begin().await?;
    let Some(target) = account(&mut tx, discord_id).await? else {
        return Err(MuteError::NoSuchUser);
    };

    if target == user_id {
        return Err(MuteError::Yourself);
    }

    let inserted = sqlx::query!(
        "INSERT INTO mutes (user_id, muted_user_id, inserted_at)
              VALUES ($1, $2, $3)
         ON CONFLICT (user_id, muted_user_id) WHERE muted_user_id IS NOT NULL DO NOTHING",
        user_id,
        target,
        now
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    tx.commit().await?;
    Ok(inserted == 1)
}

/// Remove the currency mute named by a list button. False if it was already removed.
pub async fn unmute_currency_id(
    pool: &PgPool,
    user_id: i32,
    currency_id: i64,
) -> Result<bool, MuteError> {
    remove(
        pool,
        "DELETE FROM mutes WHERE user_id = $1 AND currency_id = $2",
        user_id,
        currency_id,
    )
    .await
}

/// Forgetting somebody's mute, and answering like [`unmute_currency_id`].
pub async fn unmute_user(pool: &PgPool, user_id: i32, discord_id: i64) -> Result<bool, MuteError> {
    let mut tx = pool.begin().await?;
    let Some(target) = account(&mut tx, discord_id).await? else {
        return Err(MuteError::NoSuchUser);
    };

    let removed = remove(
        &mut *tx,
        "DELETE FROM mutes WHERE user_id = $1 AND muted_user_id = $2",
        user_id,
        i64::from(target),
    )
    .await?;
    tx.commit().await?;
    Ok(removed)
}

/// One page of the caller's mutes, newest first.
///
/// The currencies are joined rather than looked up one by one: a page is five rows and the
/// screen shows each one's name and unit, which are the same two columns the balance list reads
/// out of the same table.
pub async fn page(pool: &PgPool, user_id: i32, page: i64, limit: i64) -> Result<Mutes, MuteError> {
    let page = page.max(1);

    let total = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM mutes WHERE user_id = $1",
        user_id
    )
    .fetch_one(pool)
    .await?;

    let rows = sqlx::query!(
        "SELECT mutes.currency_id, currencies.unit, currencies.name, users.discord_id
           FROM mutes
           LEFT JOIN currencies ON currencies.id = mutes.currency_id
           LEFT JOIN users ON users.id = mutes.muted_user_id
          WHERE mutes.user_id = $1
          ORDER BY mutes.inserted_at DESC
          LIMIT $2
         OFFSET $3",
        user_id,
        limit,
        limit * (page - 1)
    )
    .fetch_all(pool)
    .await?;

    let last_page = ((total + limit - 1) / limit).max(1);

    Ok(Mutes {
        mutes: rows
            .into_iter()
            .filter_map(
                |row| match (row.currency_id, row.unit, row.name, row.discord_id) {
                    (Some(id), unit, name, _) => Some(Target::Currency {
                        id,
                        unit: unit.unwrap_or_default(),
                        name: name.unwrap_or_default(),
                    }),
                    (None, _, _, Some(discord_id)) => Some(Target::User { discord_id }),
                    // A row with no target is one the table's own constraint refuses.
                    (None, _, _, None) => None,
                },
            )
            .collect(),
        total,
        page,
        first: (page != 1).then_some(1),
        prev: (page != 1).then_some(page - 1),
        next: (page < last_page).then_some(page + 1),
        last: (page < last_page).then_some(last_page),
    })
}

/// The one statement an unmute is: the two differ in the column they ask about, and nothing else
/// about them does.
async fn remove<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    executor: E,
    statement: &'static str,
    user_id: i32,
    target: i64,
) -> Result<bool, MuteError> {
    let removed = sqlx::query(statement)
        .bind(user_id)
        .bind(target)
        .execute(executor)
        .await?
        .rows_affected();

    Ok(removed == 1)
}

/// Keep the target's identity stable through the mute change. A bot binding may
/// retire or reassign the candidate while its lock is awaited: resolve again
/// with a fresh snapshot in that case, without creating an absent account.
async fn account(conn: &mut PgConnection, discord_id: i64) -> Result<Option<i32>, MuteError> {
    loop {
        let Some(id) =
            sqlx::query_scalar!("SELECT id FROM users WHERE discord_id = $1", discord_id)
                .fetch_optional(&mut *conn)
                .await?
        else {
            return Ok(None);
        };
        if sqlx::query_scalar!(
            "SELECT id FROM users WHERE id = $1 AND discord_id = $2 FOR KEY SHARE",
            id,
            discord_id
        )
        .fetch_optional(&mut *conn)
        .await?
        .is_some()
        {
            return Ok(Some(id));
        }
    }
}
