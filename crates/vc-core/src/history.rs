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
//! What a screen asks for is one page of one of them, which is why [`Page`] is here rather than
//! spelled out twice: the four arrows are the same arithmetic for both.

use sqlx::PgPool;
use time::PrimitiveDateTime;

use crate::Result;

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
/// the guild's.
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

/// One page of what a Discord user was paid and what they paid, newest first.
///
/// `unit` narrows it to one currency and `counterparty` to one person on either side — the two
/// filters the claim list has, for the reason it has them: a ledger grows and a person looks at
/// it for one thing at a time.
pub async fn payments(
    pool: &PgPool,
    discord_id: i64,
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
          WHERE (sender.discord_id = $1 OR receiver.discord_id = $1)
            AND ($2::text IS NULL OR currencies.unit = $2)
            AND ($3::bigint IS NULL
                 OR sender.discord_id = $3 OR receiver.discord_id = $3)",
        discord_id,
        unit,
        counterparty
    )
    .fetch_one(pool)
    .await?;

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
          WHERE (sender.discord_id = $1 OR receiver.discord_id = $1)
            AND ($2::text IS NULL OR currencies.unit = $2)
            AND ($3::bigint IS NULL
                 OR sender.discord_id = $3 OR receiver.discord_id = $3)
          ORDER BY history.id DESC
          LIMIT $4
         OFFSET $5",
        discord_id,
        unit,
        counterparty,
        limit,
        limit * (page - 1)
    )
    .fetch_all(pool)
    .await?;

    let payments = rows
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
        .collect();

    Ok(paged(payments, total, page, limit))
}

/// One page of what one guild's pool has issued, newest first.
///
/// The guild is where the currency is rather than where the row is: `currency_given_histories`
/// has no guild column, and a currency belongs to one guild, which is what makes the join the
/// filter. `receiver` narrows it to what one member was given.
pub async fn issuances(
    pool: &PgPool,
    guild_id: i64,
    receiver: Option<i64>,
    page: i64,
    limit: i64,
) -> Result<Page<Issuance>> {
    let page = page.max(1);

    let total = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\"
           FROM currency_given_histories given
           JOIN currencies ON currencies.id = given.currency_id
           JOIN users receiver ON receiver.id = given.receiver_id
          WHERE currencies.guild_id = $1
            AND ($2::bigint IS NULL OR receiver.discord_id = $2)",
        guild_id,
        receiver
    )
    .fetch_one(pool)
    .await?;

    let rows = sqlx::query!(
        "SELECT given.id, given.amount AS \"amount!\",
                COALESCE(given.\"time\", given.inserted_at) AS \"time!\",
                receiver.discord_id AS receiver_discord_id,
                currencies.unit
           FROM currency_given_histories given
           JOIN currencies ON currencies.id = given.currency_id
           JOIN users receiver ON receiver.id = given.receiver_id
          WHERE currencies.guild_id = $1
            AND ($2::bigint IS NULL OR receiver.discord_id = $2)
          ORDER BY given.id DESC
          LIMIT $3
         OFFSET $4",
        guild_id,
        receiver,
        limit,
        limit * (page - 1)
    )
    .fetch_all(pool)
    .await?;

    let issuances = rows
        .into_iter()
        .map(|row| Issuance {
            id: row.id,
            amount: row.amount,
            receiver_discord_id: row.receiver_discord_id,
            unit: row.unit.unwrap_or_default(),
            time: row.time,
        })
        .collect();

    Ok(paged(issuances, total, page, limit))
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
