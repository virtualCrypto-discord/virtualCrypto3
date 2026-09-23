//! The ledger behind `/issue` and `/pay`, read back.
//!
//! Two tables have recorded every movement since the Elixir: `currency_given_histories` is the
//! pool paying somebody (`/issue`), and `currency_payment_histories` is one account paying
//! another — a `/pay`, a claim being approved, and a contract's charge, which migration `0009`
//! marks with a `contract_id` the Elixir does not have. Both tables were write-only in this tree
//! until now: nothing read either of them, and the Elixir reads neither either — its
//! `/api/v1|2/…/transactions` are writers, its web has no page for them, and no command of its
//! names a history. So this module is an addition, and `docs/known-gaps.md` records it as one.
//!
//! Each ledger has two readers, and the reason is the claim list's: a screen moves to a *page*
//! and needs the count behind it, while the API resumes from a *cursor* — the id of the last row
//! it was given. One predicate, written twice, is what the claim list does with `list` and
//! `list_page`, and a filter applied after the read would answer a page that is not the page.
//!
//! Both are keyed the way the rest of this service keys things: the caller by their account, and
//! the issuance ledger by the currency whose pool paid, because that is what the API's path
//! names. A Discord screen holds a Discord id and a guild, so it looks the one it needs up first.

use sqlx::PgPool;
use time::PrimitiveDateTime;

use crate::Result;
use crate::page::Cursor;

/// How many rows one screen shows: the claim list's five, because these rows are two lines each
/// and a screen somebody scrolls is a screen they scroll past.
pub const PER_PAGE: i64 = 5;

/// One payment, as a screen has to say it: who paid whom, how much, in what, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payment {
    pub id: i64,
    pub amount: i64,
    /// Nullable because the column is: an account made for an application has no Discord id, and
    /// one of those is who a contract's charge is paid to.
    pub sender_discord_id: Option<i64>,
    pub receiver_discord_id: Option<i64>,
    pub unit: String,
    pub time: PrimitiveDateTime,
    /// The application whose contract took this, when the row is a charge rather than a payment
    /// between people. `None` for `/pay` and for a claim's approval.
    pub contract_client_name: Option<String>,
}

/// One issuance, as a screen has to say it. There is no sender: the pool paid, and the pool is
/// the currency's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issuance {
    pub id: i64,
    pub amount: i64,
    pub receiver_discord_id: Option<i64>,
    pub unit: String,
    pub time: PrimitiveDateTime,
}

/// One page of a history, whichever history it is: the rows, the whole count, and the four
/// places the arrows move to — the shape `Balances` and `OpenContracts` each write out, written
/// once here because this module has two lists rather than one.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub rows: Vec<T>,
    /// How many there are altogether, which is the number the screen may say.
    pub total: i64,
    /// Which page this is, counting from one.
    pub page: i64,
    /// Each `None` when there is nowhere to go, which is what disables a button rather than
    /// leaving it out.
    pub first: Option<i64>,
    pub prev: Option<i64>,
    pub next: Option<i64>,
    pub last: Option<i64>,
}

/// One page of what one account was paid and what it paid, newest first.
///
/// `unit` narrows it to one currency and `counterparty` to one Discord user on either side — the
/// two filters the claim list has, for the reason it has them: a ledger grows, and a person
/// looks at it for one thing at a time.
pub async fn payments(
    pool: &PgPool,
    user_id: i32,
    unit: Option<&str>,
    counterparty: Option<i64>,
    page: i64,
    limit: i64,
) -> Result<Page<Payment>> {
    let page = page.max(1);

    let total = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\"
           FROM currency_payment_histories history
           JOIN currencies ON currencies.id = history.currency_id
           JOIN users sender ON sender.id = history.sender_id
           JOIN users receiver ON receiver.id = history.receiver_id
          WHERE (history.sender_id = $1 OR history.receiver_id = $1)
            AND ($2::text IS NULL OR currencies.unit = $2)
            AND ($3::bigint IS NULL
                 OR sender.discord_id = $3 OR receiver.discord_id = $3)",
        i64::from(user_id),
        unit,
        counterparty
    )
    .fetch_one(pool)
    .await?;

    let rows = query_payments(
        pool,
        user_id,
        unit,
        counterparty,
        None,
        None,
        Some(limit),
        limit * (page - 1),
    )
    .await?;

    Ok(paged(rows, total, page, limit))
}

/// The same ledger, resumed from a cursor: the API's reader, where `next` is the id of the last
/// row the caller was given and `on_next` includes the row it names. An absent `limit` is every
/// row, as it is for every list here — the route always asks for one.
pub async fn payments_after(
    pool: &PgPool,
    user_id: i32,
    unit: Option<&str>,
    counterparty: Option<i64>,
    cursor: Cursor,
    limit: Option<i64>,
) -> Result<Vec<Payment>> {
    let (next, on_next) = cursors(cursor);

    query_payments(pool, user_id, unit, counterparty, next, on_next, limit, 0).await
}

/// One page of what one currency's pool has issued, newest first.
///
/// The filter is the currency rather than the guild: `currency_given_histories` has no guild
/// column, a currency belongs to one guild, and the guild is what a Discord screen has to look
/// up. `receiver` narrows it to what one person was given.
pub async fn issuances(
    pool: &PgPool,
    currency_id: i64,
    receiver: Option<i64>,
    page: i64,
    limit: i64,
) -> Result<Page<Issuance>> {
    let page = page.max(1);

    let total = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\"
           FROM currency_given_histories given
           JOIN users receiver ON receiver.id = given.receiver_id
          WHERE given.currency_id = $1
            AND ($2::bigint IS NULL OR receiver.discord_id = $2)",
        currency_id,
        receiver
    )
    .fetch_one(pool)
    .await?;

    let rows = query_issuances(
        pool,
        currency_id,
        receiver,
        None,
        None,
        Some(limit),
        limit * (page - 1),
    )
    .await?;

    Ok(paged(rows, total, page, limit))
}

/// The same ledger, resumed from a cursor.
pub async fn issuances_after(
    pool: &PgPool,
    currency_id: i64,
    receiver: Option<i64>,
    cursor: Cursor,
    limit: Option<i64>,
) -> Result<Vec<Issuance>> {
    let (next, on_next) = cursors(cursor);

    query_issuances(pool, currency_id, receiver, next, on_next, limit, 0).await
}

/// The currency whose pool belongs to one guild, which is what a Discord screen has to resolve
/// before it can ask for the guild's issuance ledger — and what `/issue` resolves before it can
/// spend from it. A guild with more than one names the one `/issue` would spend from.
pub async fn guild_currency(pool: &PgPool, guild_id: i64) -> Result<Option<i64>> {
    Ok(
        sqlx::query_scalar!("SELECT id FROM currencies WHERE guild_id = $1", guild_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// The payments query itself, which the two readers above differ about only in how they resume
/// and how much they skip.
#[allow(clippy::too_many_arguments)]
async fn query_payments(
    pool: &PgPool,
    user_id: i32,
    unit: Option<&str>,
    counterparty: Option<i64>,
    next: Option<i64>,
    on_next: Option<i64>,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<Payment>> {
    let rows = sqlx::query!(
        "SELECT history.id, history.amount AS \"amount!\",
                COALESCE(history.\"time\", history.inserted_at) AS \"time!\",
                sender.discord_id AS sender_discord_id,
                receiver.discord_id AS receiver_discord_id,
                currencies.unit,
                applications.client_name AS client_name
           FROM currency_payment_histories history
           JOIN currencies ON currencies.id = history.currency_id
           JOIN users sender ON sender.id = history.sender_id
           JOIN users receiver ON receiver.id = history.receiver_id
           LEFT JOIN contracts ON contracts.id = history.contract_id
           LEFT JOIN applications ON applications.id = contracts.application_id
          WHERE (history.sender_id = $1 OR history.receiver_id = $1)
            AND ($2::text IS NULL OR currencies.unit = $2)
            AND ($3::bigint IS NULL
                 OR sender.discord_id = $3 OR receiver.discord_id = $3)
            AND ($4::bigint IS NULL OR history.id < $4)
            AND ($5::bigint IS NULL OR history.id <= $5)
          ORDER BY history.id DESC
          LIMIT $6
         OFFSET $7",
        i64::from(user_id),
        unit,
        counterparty,
        next,
        on_next,
        limit,
        offset
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| Payment {
            id: row.id,
            amount: row.amount,
            sender_discord_id: row.sender_discord_id,
            receiver_discord_id: row.receiver_discord_id,
            unit: row.unit.unwrap_or_default(),
            time: row.time,
            contract_client_name: row.client_name,
        })
        .collect())
}

/// The issuance query itself, for the same reason.
async fn query_issuances(
    pool: &PgPool,
    currency_id: i64,
    receiver: Option<i64>,
    next: Option<i64>,
    on_next: Option<i64>,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<Issuance>> {
    let rows = sqlx::query!(
        "SELECT given.id, given.amount AS \"amount!\",
                COALESCE(given.\"time\", given.inserted_at) AS \"time!\",
                receiver.discord_id AS receiver_discord_id,
                currencies.unit
           FROM currency_given_histories given
           JOIN currencies ON currencies.id = given.currency_id
           JOIN users receiver ON receiver.id = given.receiver_id
          WHERE given.currency_id = $1
            AND ($2::bigint IS NULL OR receiver.discord_id = $2)
            AND ($3::bigint IS NULL OR given.id < $3)
            AND ($4::bigint IS NULL OR given.id <= $4)
          ORDER BY given.id DESC
          LIMIT $5
         OFFSET $6",
        currency_id,
        receiver,
        next,
        on_next,
        limit,
        offset
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| Issuance {
            id: row.id,
            amount: row.amount,
            receiver_discord_id: row.receiver_discord_id,
            unit: row.unit.unwrap_or_default(),
            time: row.time,
        })
        .collect())
}

/// `next` and `on_next` as the two comparison values they are. `vc_core::contract` keeps its own
/// copy for the same reason: a query needs both, and neither module wants the other's.
fn cursors(cursor: Cursor) -> (Option<i64>, Option<i64>) {
    match cursor {
        Cursor::First => (None, None),
        Cursor::Next(value) => (Some(value), None),
        Cursor::OnNext(value) => (None, Some(value)),
    }
}

/// A page of rows with the four moves its arrows make.
fn paged<T>(rows: Vec<T>, total: i64, page: i64, limit: i64) -> Page<T> {
    let last_page = ((total + limit - 1) / limit).max(1);

    Page {
        rows,
        total,
        page,
        // One and n - 1, or nothing on the first page; n + 1 and the last page, or nothing on
        // the last. The claim list's arithmetic, which every list here has.
        first: (page != 1).then_some(1),
        prev: (page != 1).then_some(page - 1),
        next: (page < last_page).then_some(page + 1),
        last: (page < last_page).then_some(last_page),
    }
}
