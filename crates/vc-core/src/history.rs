//! The ledgers behind `/issue` and `/pay`, read back.
//!
//! Two tables have recorded every movement since the Elixir: `currency_given_histories` is the
//! pool paying somebody (`/issue`), and `currency_payment_histories` is one account paying
//! another — a `/pay`, a claim being approved, and a contract's three movements, which migration
//! `0009` marks with a `contract_id` the Elixir does not have. Both tables were write-only in this
//! tree until now: nothing read either of them, and the Elixir reads neither either — its
//! `/api/v1|2/…/transactions` are writers, its web has no page for them, and no command of its
//! names a history. So this module is an addition, and `docs/known-gaps.md` records it as one.
//!
//! A person's ledger is both tables at once. What arrived in somebody's wallet is a payment to
//! them, a contract returning what was left of their lock, and an issuance — the pool paying them
//! is money arriving like any other — so `/history pay` and `GET /api/v2/users/@me/transactions`
//! read the two together, newest first ([`Movement`], [`Place`]). The guild's issuance ledger is
//! one table and one screen, because it is the guild's rather than a person's ([`issuances`]).
//!
//! Each ledger has two readers, and the reason is the claim list's: a screen moves to a *page*
//! and needs the count behind it, while the API resumes from a *cursor* — the place the last row
//! it was given holds. One predicate, written twice, is what the claim list does with `list` and
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
    /// one of those is who a contract's charge is paid to — and because a contract row names the
    /// escrow rather than an account on one side.
    pub sender_discord_id: Option<i64>,
    pub receiver_discord_id: Option<i64>,
    pub unit: String,
    pub time: PrimitiveDateTime,
    /// The application whose contract this belongs to, when the row is a contract movement:
    /// `None` for `/pay` and for a claim's approval.
    pub contract_client_name: Option<String>,
    /// Which contract movement this is, when it is one: `Some("lock")` for money a party locked by
    /// approving, `Some("return")` for money the contract gave back, and `None` for a plain
    /// payment. `Some("charge")` is a contract payment received by this wallet;
    /// spending an escrow never appears as another debit from the payer's wallet.
    pub event: Option<&'static str>,
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

/// One movement of a person's wallet, whichever ledger recorded it.
///
/// The two are one list because to the person they are one thing: money arriving, or money
/// leaving. Which table a row is in says who the other end was — an account, a contract's escrow,
/// or the pool — rather than whether it was the wallet's money.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Movement {
    /// A row of `currency_payment_histories`: a `/pay`, a claim's approval, or one of a
    /// contract's three movements.
    Payment(Payment),
    /// A row of `currency_given_histories`: the pool paying this person.
    Issuance(Issuance),
}

impl Movement {
    /// Where this row sits in the merged order, which is what a cursor spells.
    pub fn place(&self) -> Place {
        Place {
            time: match self {
                Movement::Payment(payment) => payment.time,
                Movement::Issuance(issuance) => issuance.time,
            },
            kind: self.kind(),
            id: match self {
                Movement::Payment(payment) => payment.id,
                Movement::Issuance(issuance) => issuance.id,
            },
        }
    }

    fn kind(&self) -> i16 {
        match self {
            Movement::Payment(_) => PAYMENT,
            Movement::Issuance(_) => ISSUANCE,
        }
    }
}

/// The two ledgers' numbers in the merged order: payments are 0 and issuances are 1. The order is
/// `kind DESC`, so within one second the pool's arrival reads before the wallet's own movement,
/// and the number is what the middle field of a cursor carries.
const PAYMENT: i16 = 0;
const ISSUANCE: i16 = 1;

/// Where a row sits in the merged ledger: the `time` it carries, the ledger it is from, and its
/// id.
///
/// The two tables count their ids separately, so neither an id nor a time alone is a total order:
/// `"time"` is whole seconds and ties are ordinary, which is what the ledger and then the id break.
/// It is a string rather than one number because of that, and the endpoint's cursor is this —
/// `2026-01-01T00:00:00Z:1:5`, the timestamp the API renders, the ledger, and the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    time: PrimitiveDateTime,
    kind: i16,
    id: i64,
}

impl Place {
    /// The cursor's spelling, which is also what a caller hands back: colon separated, and read
    /// from the right because the timestamp has colons of its own.
    pub fn encode(self) -> String {
        format!("{}:{}:{}", timestamp(self.time), self.kind, self.id)
    }

    /// The same in reverse, or `None` for text that is not one of ours.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.rsplitn(3, ':');
        let id = parts.next()?.parse().ok()?;
        let kind = parts.next()?.parse().ok()?;

        if kind != PAYMENT && kind != ISSUANCE {
            return None;
        }

        Some(Self {
            time: parse_timestamp(parts.next()?)?,
            kind,
            id,
        })
    }
}

/// The timestamp as the API renders it and a cursor spells it: whole seconds, UTC.
fn timestamp(value: PrimitiveDateTime) -> String {
    let format =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

    value.format(&format).unwrap_or_default()
}

/// The same, read back.
fn parse_timestamp(text: &str) -> Option<PrimitiveDateTime> {
    let format =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

    PrimitiveDateTime::parse(text, &format).ok()
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

/// One page of what one account's wallet moved, newest first: what it paid and was paid, what a
/// contract locked and returned, and what the pool issued to it.
///
/// `unit` narrows it to one currency and `counterparty` to one Discord user on either side — the
/// two filters the claim list has, for the reason it has them: a ledger grows, and a person
/// looks at it for one thing at a time. A counterparty narrows an issuance out, because an
/// issuance is the pool's and has no other person in it to be the one asked about.
pub async fn payments(
    pool: &PgPool,
    user_id: i32,
    unit: Option<&str>,
    counterparty: Option<i64>,
    page: i64,
    limit: i64,
) -> Result<Page<Movement>> {
    let page = page.max(1);

    let total = sqlx::query_scalar!(
        // What moved this person's wallet, out of both ledgers. Out of the payment ledger it is a
        // row with no contract (a payment between people) or a contract row with a NULL side — a
        // lock (no receiver: the escrow took it) or a return (no sender: the escrow sent it). A
        // contract row with both sides set is a charge. It credits the receiver's wallet,
        // but must not count again as a debit to the party who already locked the funds.
        // Out of the issuance ledger it is every row, because the pool paid this account.
        "SELECT count(*) AS \"count!\"
           FROM (
                   SELECT history.currency_id, history.sender_id, history.receiver_id
                     FROM currency_payment_histories history
                    WHERE (history.sender_id = $1 OR history.receiver_id = $1)
                      AND (history.contract_id IS NULL
                           OR history.sender_id IS NULL
                           OR history.receiver_id IS NULL
                           OR history.receiver_id = $1)
                   UNION ALL
                   SELECT given.currency_id, NULL::bigint, given.receiver_id
                     FROM currency_given_histories given
                    WHERE given.receiver_id = $1
                      AND $3::bigint IS NULL
                 ) movement
           JOIN currencies ON currencies.id = movement.currency_id
           LEFT JOIN users sender ON sender.id = movement.sender_id
           LEFT JOIN users receiver ON receiver.id = movement.receiver_id
          WHERE ($2::text IS NULL OR currencies.unit = $2)
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
        false,
        Some(limit),
        limit * (page - 1),
    )
    .await?;

    Ok(paged(rows, total, page, limit))
}

/// The same list, resumed from a cursor: the API's reader, where `next` is the place of the last
/// row the caller was given and `on_next` includes the row it names. An absent `limit` is every
/// row, as it is for every list here — the route always asks for one.
pub async fn payments_after(
    pool: &PgPool,
    user_id: i32,
    unit: Option<&str>,
    counterparty: Option<i64>,
    cursor: Cursor<Place>,
    limit: Option<i64>,
) -> Result<Vec<Movement>> {
    let (at, inclusive) = place(cursor);

    query_payments(pool, user_id, unit, counterparty, at, inclusive, limit, 0).await
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
///
/// The merged order is the SQL's: `time` newest first, then the ledger (`kind DESC`), then the
/// id — which is total because the two tables' id sequences are separate and `time` is whole
/// seconds. The cursor is the same three values, and `<` is `next` where `<=` is `on_next`: one
/// bound and a flag rather than two parameter triples for the same place.
#[allow(clippy::too_many_arguments)]
async fn query_payments(
    pool: &PgPool,
    user_id: i32,
    unit: Option<&str>,
    counterparty: Option<i64>,
    at: Option<Place>,
    inclusive: bool,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<Movement>> {
    let at_time = at.map(|at| at.time);
    let at_kind = at.map(|at| at.kind);
    let at_id = at.map(|at| at.id);

    let rows = sqlx::query!(
        // The same predicate the count above carries, and for the same reason. Both sides of a
        // payment are outer joins because a lock or a return names the escrow — no user — on one
        // of them; an issuance has no sender at all. `kind` is `PAYMENT` or `ISSUANCE`, bound
        // rather than written out so that the two tables cannot drift apart in this statement
        // from the ones the cursor is spelled from.
        "SELECT movement.id AS \"id!\",
                movement.kind AS \"kind!\",
                movement.amount AS \"amount!\",
                movement.\"time\" AS \"time!\",
                sender.discord_id AS sender_discord_id,
                receiver.discord_id AS receiver_discord_id,
                currencies.unit,
                applications.client_name AS client_name,
                movement.contract_id IS NOT NULL AS \"contract!\",
                movement.sender_id IS NULL AS \"sender_unset!\",
                movement.receiver_id IS NULL AS \"receiver_unset!\"
           FROM (
                   SELECT history.id, $1::smallint AS kind, history.amount,
                          COALESCE(history.\"time\", history.inserted_at) AS \"time\",
                          history.currency_id, history.sender_id, history.receiver_id,
                          history.contract_id
                     FROM currency_payment_histories history
                    WHERE (history.sender_id = $3 OR history.receiver_id = $3)
                      AND (history.contract_id IS NULL
                           OR history.sender_id IS NULL
                           OR history.receiver_id IS NULL
                           OR history.receiver_id = $3)
                   UNION ALL
                   SELECT given.id, $2::smallint, given.amount,
                          COALESCE(given.\"time\", given.inserted_at),
                          given.currency_id, NULL::bigint, given.receiver_id, NULL::bigint
                     FROM currency_given_histories given
                    WHERE given.receiver_id = $3
                      AND $4::bigint IS NULL
                 ) movement
           JOIN currencies ON currencies.id = movement.currency_id
           LEFT JOIN users sender ON sender.id = movement.sender_id
           LEFT JOIN users receiver ON receiver.id = movement.receiver_id
           LEFT JOIN contracts ON contracts.id = movement.contract_id
           LEFT JOIN applications ON applications.id = contracts.application_id
          WHERE ($5::text IS NULL OR currencies.unit = $5)
            AND ($4::bigint IS NULL
                 OR sender.discord_id = $4 OR receiver.discord_id = $4)
            AND ($6::timestamp IS NULL
                 OR (movement.\"time\", movement.kind, movement.id)
                        < ($6, $7::smallint, $8::bigint)
                 OR ($9::boolean
                     AND (movement.\"time\", movement.kind, movement.id)
                            = ($6, $7::smallint, $8::bigint)))
          ORDER BY movement.\"time\" DESC, movement.kind DESC, movement.id DESC
          LIMIT $10
         OFFSET $11",
        PAYMENT,
        ISSUANCE,
        i64::from(user_id),
        counterparty,
        unit,
        at_time,
        at_kind,
        at_id,
        inclusive,
        limit,
        offset
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let unit = row.unit.unwrap_or_default();

            if row.kind == ISSUANCE {
                Movement::Issuance(Issuance {
                    id: row.id,
                    amount: row.amount,
                    receiver_discord_id: row.receiver_discord_id,
                    unit,
                    time: row.time,
                })
            } else {
                Movement::Payment(Payment {
                    id: row.id,
                    amount: row.amount,
                    sender_discord_id: row.sender_discord_id,
                    receiver_discord_id: row.receiver_discord_id,
                    unit,
                    time: row.time,
                    contract_client_name: row.client_name,
                    event: event(row.contract, row.sender_unset, row.receiver_unset),
                })
            }
        })
        .collect())
}

/// Which contract movement a wallet row is, when it is one: a contract row with no receiver is a
/// lock and one with no sender is a return. A charge is visible only to its receiver.
fn event(contract: bool, sender_unset: bool, receiver_unset: bool) -> Option<&'static str> {
    match (contract, sender_unset, receiver_unset) {
        (true, _, true) => Some("lock"),
        (true, true, _) => Some("return"),
        (true, false, false) => Some("charge"),
        _ => None,
    }
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

/// The place a merged cursor names, and whether the row it names is in the page that follows:
/// `next` resumes after it and `on_next` from it.
fn place(cursor: Cursor<Place>) -> (Option<Place>, bool) {
    match cursor {
        Cursor::First => (None, false),
        Cursor::Next(at) => (Some(at), false),
        Cursor::OnNext(at) => (Some(at), true),
    }
}

/// `next` and `on_next` as the two comparison values they are, for a list whose cursor is an id.
/// `vc_core::contract` keeps its own copy for the same reason: a query needs both, and neither
/// module wants the other's.
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
