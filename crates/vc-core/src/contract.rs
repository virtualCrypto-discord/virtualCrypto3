//! Contracts: an application operating a user's currency, on the strength of the
//! parties' approvals.
//!
//! A contract names the users whose currency an application wants to operate and
//! what each of them locks by approving. Locking is the movement `transfer`
//! performs minus the receiving balance: the party's asset row is decremented in
//! the same transaction that writes their `remaining`, and the `assets` trigger
//! deletes a balance that reaches zero. Spending and refunding are that movement
//! again — out to a receiver, or back to the party — so **the escrow is the
//! parties' remainders** and there is no second ledger to keep in step with the
//! first.

use sqlx::{PgConnection, PgPool};
use time::{Duration, OffsetDateTime, PrimitiveDateTime};

/// How many parties one contract may name.
pub const MAX_PARTIES: usize = 50;
/// How long a temporary contract may run: a year, in seconds.
pub const MAX_EXPIRES_IN: i64 = 365 * 24 * 60 * 60;

#[derive(Debug)]
pub enum ContractError {
    /// No such contract, or the caller is not one of the people it names. The
    /// two are one answer on purpose: a caller that is not a party must not be
    /// able to ask which ids exist.
    NotFound,
    NotFoundCurrency,
    InvalidAmount,
    /// No parties, too many, or one named twice.
    InvalidParties,
    InvalidExpiresIn,
    /// The party cannot cover the amount they would lock.
    NotEnoughAmount,
    /// The contract is not in a state this belongs to — already decided, or a
    /// temporary contract that is still running and cannot be withdrawn from.
    InvalidStatus,
    /// Its deadline has passed.
    Expired,
    /// The contract fixes the receiver, and the payment names someone else.
    ReceiverIsFixed,
    Database(sqlx::Error),
}

/// One user the contract names, and what they lock by approving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewParty {
    pub discord_id: i64,
    pub amount: i64,
}

/// A party as a read answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Party {
    pub discord_id: i64,
    /// What approving locks.
    pub amount: i64,
    /// What the application has not spent of it.
    pub remaining: i64,
    /// `pending`, `approved`, `refused`, or `withdrawn`.
    pub status: String,
}

/// A contract with its parties, which is what every read answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub id: i64,
    pub application_id: i64,
    pub unit: Option<String>,
    pub guild_id: Option<i64>,
    /// When set, the only Discord user the locked money may be paid to.
    pub receiver_discord_id: Option<i64>,
    /// When set, the end of a temporary contract.
    pub expires_at: Option<PrimitiveDateTime>,
    pub status: String,
    /// What the application may still spend: the parties' remainders, summed.
    pub remaining: i64,
    pub parties: Vec<Party>,
}

/// What a payment moved, and what is left to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Payed {
    pub amount: i64,
    pub remaining: i64,
}

/// One party, and what they hold of the contract's currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartyBalance {
    pub discord_id: i64,
    pub amount: i64,
}

fn at(now: OffsetDateTime) -> PrimitiveDateTime {
    PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second()
}

/// An application proposing to operate a currency on named users' behalf.
///
/// Nothing is locked and nothing is agreed here: this is the question the
/// parties answer, and each of them locks their own amount by approving. The
/// parties are Discord users rather than accounts, because a contract may name
/// someone who has never used this service — their account is created when they
/// log in to answer.
///
/// `expires_in` is how long the delegation runs, in seconds, and absent is a
/// permanent one. A receiver, when given, is the only user the money may be paid
/// to; absent means the application names one payment by payment.
pub async fn create(
    pool: &PgPool,
    application_id: i64,
    unit: &str,
    parties: &[NewParty],
    receiver_discord_id: Option<i64>,
    expires_in: Option<i64>,
    now: OffsetDateTime,
) -> std::result::Result<i64, ContractError> {
    if parties.is_empty() || parties.len() > MAX_PARTIES {
        return Err(ContractError::InvalidParties);
    }

    if parties.iter().any(|party| party.amount <= 0) {
        return Err(ContractError::InvalidAmount);
    }

    // One amount per person: a second row for the same user would be a second
    // thing to approve rather than a larger one.
    let mut named: Vec<i64> = parties.iter().map(|party| party.discord_id).collect();
    named.sort_unstable();
    named.dedup();
    if named.len() != parties.len() {
        return Err(ContractError::InvalidParties);
    }

    if expires_in.is_some_and(|seconds| seconds <= 0 || seconds > MAX_EXPIRES_IN) {
        return Err(ContractError::InvalidExpiresIn);
    }

    let now = at(now);
    let expires_at = expires_in.map(|seconds| now + Duration::seconds(seconds));

    let mut tx = pool.begin().await.map_err(ContractError::Database)?;

    let currency_id = sqlx::query_scalar!("SELECT id FROM currencies WHERE unit = $1", unit)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ContractError::Database)?
        .ok_or(ContractError::NotFoundCurrency)?;

    let contract_id = sqlx::query_scalar!(
        "INSERT INTO contracts
             (application_id, currency_id, receiver_discord_id, expires_at, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $5)
         RETURNING id",
        application_id,
        currency_id,
        receiver_discord_id,
        expires_at,
        now
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    let amounts: Vec<i64> = parties.iter().map(|party| party.amount).collect();

    sqlx::query!(
        "INSERT INTO contract_parties (contract_id, discord_id, amount, inserted_at, updated_at)
         SELECT $1, t.discord_id, t.amount, $4, $4
           FROM UNNEST($2::bigint[], $3::bigint[]) AS t(discord_id, amount)",
        contract_id,
        &named,
        &amounts,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    tx.commit().await.map_err(ContractError::Database)?;

    Ok(contract_id)
}

/// One contract and its parties, or `None` when there is no such contract.
pub async fn find(
    pool: &PgPool,
    contract_id: i64,
) -> std::result::Result<Option<Contract>, ContractError> {
    let row = sqlx::query_as!(
        ContractRow,
        "SELECT c.id, c.application_id, c.receiver_discord_id, c.expires_at,
                c.status AS \"status!\", currencies.unit, currencies.guild_id
           FROM contracts c
           JOIN currencies ON currencies.id = c.currency_id
          WHERE c.id = $1",
        contract_id
    )
    .fetch_optional(pool)
    .await
    .map_err(ContractError::Database)?;

    match row {
        Some(row) => Ok(Some(read_parties(pool, row).await?)),
        None => Ok(None),
    }
}

/// The contracts one application wrote.
pub async fn of_application(
    pool: &PgPool,
    application_id: i64,
) -> std::result::Result<Vec<Contract>, ContractError> {
    let rows = sqlx::query_as!(
        ContractRow,
        "SELECT c.id, c.application_id, c.receiver_discord_id, c.expires_at,
                c.status AS \"status!\", currencies.unit, currencies.guild_id
           FROM contracts c
           JOIN currencies ON currencies.id = c.currency_id
          WHERE c.application_id = $1
          ORDER BY c.id DESC",
        application_id
    )
    .fetch_all(pool)
    .await
    .map_err(ContractError::Database)?;

    read_all(pool, rows).await
}

/// The contracts one user is named in.
pub async fn of_party(
    pool: &PgPool,
    discord_id: i64,
) -> std::result::Result<Vec<Contract>, ContractError> {
    let rows = sqlx::query_as!(
        ContractRow,
        "SELECT c.id, c.application_id, c.receiver_discord_id, c.expires_at,
                c.status AS \"status!\", currencies.unit, currencies.guild_id
           FROM contracts c
           JOIN currencies ON currencies.id = c.currency_id
           JOIN contract_parties p ON p.contract_id = c.id
          WHERE p.discord_id = $1
          ORDER BY c.id DESC",
        discord_id
    )
    .fetch_all(pool)
    .await
    .map_err(ContractError::Database)?;

    read_all(pool, rows).await
}

/// The parties of one contract, read for a row already in hand.
async fn read_parties(
    pool: &PgPool,
    row: ContractRow,
) -> std::result::Result<Contract, ContractError> {
    let parties = parties_of(pool, &[row.id])
        .await?
        .into_iter()
        .next()
        .map(|(_, parties)| parties)
        .unwrap_or_default();

    Ok(with_parties(row, parties))
}

/// The parties of several contracts, in one statement rather than one per
/// contract: a list of fifty contracts is two round trips, not fifty-one.
async fn read_all(
    pool: &PgPool,
    rows: Vec<ContractRow>,
) -> std::result::Result<Vec<Contract>, ContractError> {
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let mut parties = parties_of(pool, &ids).await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let mine = parties
                .iter()
                .position(|(contract_id, _)| *contract_id == row.id)
                .map(|at| parties.remove(at).1)
                .unwrap_or_default();

            with_parties(row, mine)
        })
        .collect())
}

struct ContractRow {
    id: i64,
    application_id: i64,
    receiver_discord_id: Option<i64>,
    expires_at: Option<PrimitiveDateTime>,
    status: String,
    unit: Option<String>,
    guild_id: Option<i64>,
}

/// Every party of every contract named, as `(contract_id, parties)`.
async fn parties_of(
    pool: &PgPool,
    contract_ids: &[i64],
) -> std::result::Result<Vec<(i64, Vec<Party>)>, ContractError> {
    if contract_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows = sqlx::query_as!(
        PartyRow,
        "SELECT contract_id, discord_id, amount, remaining, status AS \"status!\"
           FROM contract_parties
          WHERE contract_id = ANY($1)
          ORDER BY contract_id, id",
        contract_ids
    )
    .fetch_all(pool)
    .await
    .map_err(ContractError::Database)?;

    let mut grouped: Vec<(i64, Vec<Party>)> = Vec::new();

    for row in rows {
        match grouped.last_mut() {
            Some((contract_id, parties)) if *contract_id == row.contract_id => {
                parties.push(party(row));
            }
            _ => grouped.push((row.contract_id, vec![party(row)])),
        }
    }

    Ok(grouped)
}

fn party(row: PartyRow) -> Party {
    Party {
        discord_id: row.discord_id,
        amount: row.amount,
        remaining: row.remaining,
        status: row.status,
    }
}

struct PartyRow {
    contract_id: i64,
    discord_id: i64,
    amount: i64,
    remaining: i64,
    status: String,
}

fn with_parties(row: ContractRow, parties: Vec<Party>) -> Contract {
    Contract {
        id: row.id,
        application_id: row.application_id,
        unit: row.unit,
        guild_id: row.guild_id,
        receiver_discord_id: row.receiver_discord_id,
        expires_at: row.expires_at,
        status: row.status,
        remaining: parties.iter().map(|party| party.remaining).sum(),
        parties,
    }
}

/// A party's yes: their amount is locked, and the application may spend it from
/// that moment on — the contract does not wait for the others.
///
/// One transaction. The contract row is locked first, so two parties deciding at
/// once cannot both read "everyone has approved"; theirs is next, and then their
/// balance is locked with `FOR UPDATE` before it is read, so a payment made in
/// between cannot make the lock take more than they hold.
///
/// Answers whether anything changed: approving twice is not a second decision —
/// the money is already locked — and a caller that was told once does not need
/// telling again.
pub async fn approve(
    pool: &PgPool,
    contract_id: i64,
    user_id: i32,
    now: OffsetDateTime,
) -> std::result::Result<bool, ContractError> {
    let now = at(now);
    let mut tx = pool.begin().await.map_err(ContractError::Database)?;

    let contract = lock_contract(&mut tx, contract_id).await?;

    // The caller first, and their own row before the contract's state: whether
    // *they* have already approved is the question this endpoint is asked twice,
    // and it does not depend on whether everyone else has. A contract the last
    // approval just made active is the one that answers it most often.
    let Some(party) = lock_party(&mut tx, contract_id, user_id).await? else {
        return Err(ContractError::NotFound);
    };

    if party.status == "approved" {
        return Ok(false);
    }

    if party.status != "pending" {
        return Err(ContractError::InvalidStatus);
    }

    if contract.status != "pending" {
        return Err(ContractError::InvalidStatus);
    }

    if expired(contract.expires_at, now) {
        return Err(ContractError::Expired);
    }

    let balance = sqlx::query_scalar!(
        "SELECT amount FROM assets
          WHERE user_id = $1 AND currency_id = $2
            FOR UPDATE",
        i64::from(user_id),
        contract.currency_id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ContractError::Database)?
    .flatten()
    .unwrap_or(0);

    if balance < party.amount {
        return Err(ContractError::NotEnoughAmount);
    }

    sqlx::query!(
        "UPDATE assets SET amount = amount - $1, updated_at = $2
          WHERE user_id = $3 AND currency_id = $4",
        party.amount,
        now,
        i64::from(user_id),
        contract.currency_id
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    sqlx::query!(
        "UPDATE contract_parties
            SET status = 'approved', remaining = amount, updated_at = $2
          WHERE id = $1",
        party.id,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    let waiting = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM contract_parties
          WHERE contract_id = $1 AND status <> 'approved'",
        contract_id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    let active = waiting == 0;

    sqlx::query!(
        "UPDATE contracts SET status = $2, updated_at = $3 WHERE id = $1",
        contract_id,
        if active { "active" } else { "pending" },
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    tx.commit().await.map_err(ContractError::Database)?;

    Ok(true)
}

/// A party's no, before they have approved anything.
///
/// The contract cannot become what it was written as, so it is over: everyone
/// who had already locked their amount takes back what is left of it.
pub async fn refuse(
    pool: &PgPool,
    contract_id: i64,
    user_id: i32,
    now: OffsetDateTime,
) -> std::result::Result<bool, ContractError> {
    let now = at(now);
    let mut tx = pool.begin().await.map_err(ContractError::Database)?;

    let contract = lock_contract(&mut tx, contract_id).await?;

    if contract.status != "pending" {
        return Err(ContractError::InvalidStatus);
    }

    let Some(party) = lock_party(&mut tx, contract_id, user_id).await? else {
        return Err(ContractError::NotFound);
    };

    if party.status != "pending" {
        return Err(ContractError::InvalidStatus);
    }

    sqlx::query!(
        "UPDATE contract_parties SET status = 'refused', updated_at = $2 WHERE id = $1",
        party.id,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    cancel(&mut tx, contract_id, contract.currency_id, now)
        .await
        .map_err(ContractError::Database)?;

    tx.commit().await.map_err(ContractError::Database)?;

    Ok(true)
}

/// A party taking the delegation back.
///
/// Allowed while a permanent contract stands — that is what permanent means —
/// and after a temporary one has run out, which is when its delegation is over.
/// A temporary contract still running is the one case it is refused: the party
/// approved an application operating the money *for the period*, and the period
/// is not the application's to shorten.
pub async fn withdraw(
    pool: &PgPool,
    contract_id: i64,
    user_id: i32,
    now: OffsetDateTime,
) -> std::result::Result<bool, ContractError> {
    let now = at(now);
    let mut tx = pool.begin().await.map_err(ContractError::Database)?;

    let contract = lock_contract(&mut tx, contract_id).await?;

    if contract.status == "canceled" {
        return Err(ContractError::InvalidStatus);
    }

    if contract.expires_at.is_some_and(|expires| expires > now) {
        return Err(ContractError::InvalidStatus);
    }

    let Some(party) = lock_party(&mut tx, contract_id, user_id).await? else {
        return Err(ContractError::NotFound);
    };

    if party.status != "approved" {
        return Err(ContractError::InvalidStatus);
    }

    sqlx::query!(
        "UPDATE contract_parties SET status = 'withdrawn', updated_at = $2 WHERE id = $1",
        party.id,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    cancel(&mut tx, contract_id, contract.currency_id, now)
        .await
        .map_err(ContractError::Database)?;

    tx.commit().await.map_err(ContractError::Database)?;

    Ok(true)
}

/// The application spending what the parties locked.
///
/// One transaction. The contract row is locked first, so a withdrawal cannot
/// land between the bounds check and the spend; what the parties have left is
/// read, and then consumed **oldest approval first** — one statement, walking
/// the parties in id order and taking from each up to what the payment still
/// needs. Which party's money a payment used is not part of the contract: what
/// the parties agreed to is an amount, and FIFO is what makes every refund exact
/// without splitting anyone's remainder into fractions.
///
/// The receiver is a Discord id like the parties, and paying someone new creates
/// their account the way a payment does.
pub async fn pay(
    pool: &PgPool,
    contract_id: i64,
    application_id: i64,
    receiver_discord_id: i64,
    amount: i64,
    now: OffsetDateTime,
) -> std::result::Result<Payed, ContractError> {
    if amount <= 0 {
        return Err(ContractError::InvalidAmount);
    }

    let now = at(now);
    let mut tx = pool.begin().await.map_err(ContractError::Database)?;

    let contract = lock_contract(&mut tx, contract_id).await?;

    if contract.application_id != application_id {
        return Err(ContractError::NotFound);
    }

    if contract.status == "canceled" {
        return Err(ContractError::InvalidStatus);
    }

    if expired(contract.expires_at, now) {
        return Err(ContractError::Expired);
    }

    if contract
        .receiver_discord_id
        .is_some_and(|fixed| fixed != receiver_discord_id)
    {
        return Err(ContractError::ReceiverIsFixed);
    }

    let remaining = sqlx::query_scalar!(
        "SELECT COALESCE(sum(remaining), 0)::bigint AS \"remaining!\"
           FROM contract_parties WHERE contract_id = $1",
        contract_id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    if remaining < amount {
        return Err(ContractError::NotEnoughAmount);
    }

    let taken = sqlx::query!(
        "WITH ordered AS (
             SELECT p.id, u.id AS sender_id, p.remaining,
                    COALESCE(sum(p.remaining) OVER (ORDER BY p.id
                              ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0)::bigint
                        AS before
               FROM contract_parties p
               JOIN users u ON u.discord_id = p.discord_id
              WHERE p.contract_id = $1 AND p.remaining > 0
         ), spent AS (
             -- What this party pays towards the payment: nothing once the ones
             -- before it have covered it, everything they left when it does not
             -- reach them, and the rest of the payment in between.
             SELECT id, sender_id,
                    CASE
                        WHEN before >= $2::bigint THEN 0::bigint
                        WHEN before + remaining <= $2::bigint THEN remaining
                        ELSE $2::bigint - before
                    END AS amount
               FROM ordered
         )
         UPDATE contract_parties p
            SET remaining = p.remaining - spent.amount, updated_at = $3
           FROM spent
          WHERE p.id = spent.id AND spent.amount > 0
        RETURNING spent.sender_id AS \"sender_id!\", spent.amount AS \"amount!\"",
        contract_id,
        amount,
        now
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    let receiver = crate::user::insert_if_not_exists(&mut tx, receiver_discord_id)
        .await
        .map_err(ContractError::Database)?;

    sqlx::query!(
        "INSERT INTO assets (user_id, currency_id, amount, inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)
         ON CONFLICT (user_id, currency_id)
         DO UPDATE SET amount = assets.amount + EXCLUDED.amount,
                       updated_at = EXCLUDED.updated_at",
        i64::from(receiver.id),
        contract.currency_id,
        amount,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    // The ledger records what moved and from whom: one row per party whose
    // remainder was drawn on, which is the money's own path into the receiver's
    // balance rather than a note that an application paid.
    let amounts: Vec<i64> = taken.iter().map(|row| row.amount).collect();
    let senders: Vec<i64> = taken.iter().map(|row| i64::from(row.sender_id)).collect();

    sqlx::query!(
        "INSERT INTO currency_payment_histories
             (amount, sender_id, receiver_id, currency_id, \"time\", inserted_at, updated_at)
         SELECT t.amount, t.sender_id, $3, $4, $5, $5, $5
           FROM UNNEST($1::bigint[], $2::bigint[]) AS t(amount, sender_id)",
        &amounts,
        &senders,
        i64::from(receiver.id),
        contract.currency_id,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    tx.commit().await.map_err(ContractError::Database)?;

    Ok(Payed {
        amount,
        remaining: remaining - amount,
    })
}

/// What each party holds of the contract's currency, which is the balance read
/// an approval gives the application: it cannot decide what to pay out without
/// knowing what the people it pays hold.
///
/// A party with no balance is a zero rather than an absence: the `assets` trigger
/// deletes a row that reaches zero, and the application asked about a person
/// rather than about a row.
pub async fn party_balances(
    pool: &PgPool,
    contract_id: i64,
    application_id: i64,
) -> std::result::Result<Vec<PartyBalance>, ContractError> {
    let contract = find(pool, contract_id)
        .await?
        .ok_or(ContractError::NotFound)?;

    if contract.application_id != application_id {
        return Err(ContractError::NotFound);
    }

    let currency_id = currency_of(pool, contract_id).await?;

    let rows = sqlx::query!(
        "SELECT p.discord_id, COALESCE(a.amount, 0) AS \"amount!\"
           FROM contract_parties p
           LEFT JOIN users u ON u.discord_id = p.discord_id
           LEFT JOIN assets a ON a.user_id = u.id AND a.currency_id = $2
          WHERE p.contract_id = $1
          ORDER BY p.id",
        contract_id,
        currency_id
    )
    .fetch_all(pool)
    .await
    .map_err(ContractError::Database)?;

    Ok(rows
        .into_iter()
        .map(|row| PartyBalance {
            discord_id: row.discord_id,
            amount: row.amount,
        })
        .collect())
}

async fn currency_of(pool: &PgPool, contract_id: i64) -> std::result::Result<i64, ContractError> {
    sqlx::query_scalar!(
        "SELECT currency_id FROM contracts WHERE id = $1",
        contract_id
    )
    .fetch_optional(pool)
    .await
    .map_err(ContractError::Database)?
    .ok_or(ContractError::NotFound)
}

fn expired(expires_at: Option<PrimitiveDateTime>, now: PrimitiveDateTime) -> bool {
    expires_at.is_some_and(|expires| expires <= now)
}

struct LockedContract {
    application_id: i64,
    currency_id: i64,
    receiver_discord_id: Option<i64>,
    expires_at: Option<PrimitiveDateTime>,
    status: String,
}

/// The contract, locked for the rest of the transaction.
async fn lock_contract(
    tx: &mut PgConnection,
    contract_id: i64,
) -> std::result::Result<LockedContract, ContractError> {
    let row = sqlx::query!(
        "SELECT application_id, currency_id, receiver_discord_id, expires_at,
                status AS \"status!\"
           FROM contracts WHERE id = $1
             FOR UPDATE",
        contract_id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ContractError::Database)?
    .ok_or(ContractError::NotFound)?;

    Ok(LockedContract {
        application_id: row.application_id,
        currency_id: row.currency_id,
        receiver_discord_id: row.receiver_discord_id,
        expires_at: row.expires_at,
        status: row.status,
    })
}

struct LockedParty {
    id: i64,
    amount: i64,
    status: String,
}

/// The caller's party row, locked, or `None` when they are not one of the users
/// the contract names.
async fn lock_party(
    tx: &mut PgConnection,
    contract_id: i64,
    user_id: i32,
) -> std::result::Result<Option<LockedParty>, ContractError> {
    let row = sqlx::query!(
        "SELECT p.id, p.amount, p.status AS \"status!\"
           FROM contract_parties p
           JOIN users u ON u.discord_id = p.discord_id
          WHERE p.contract_id = $1 AND u.id = $2
            FOR UPDATE OF p",
        contract_id,
        user_id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    Ok(row.map(|row| LockedParty {
        id: row.id,
        amount: row.amount,
        status: row.status,
    }))
}

/// The end of a contract: what is left of every party's lock goes back to them,
/// and the contract joins the ones that are over.
async fn cancel(
    tx: &mut PgConnection,
    contract_id: i64,
    currency_id: i64,
    now: PrimitiveDateTime,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO assets (user_id, currency_id, amount, inserted_at, updated_at)
         SELECT u.id, $2, p.remaining, $3, $3
           FROM contract_parties p
           JOIN users u ON u.discord_id = p.discord_id
          WHERE p.contract_id = $1 AND p.remaining > 0
         ON CONFLICT (user_id, currency_id)
         DO UPDATE SET amount = assets.amount + EXCLUDED.amount,
                       updated_at = EXCLUDED.updated_at",
        contract_id,
        currency_id,
        now
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        "UPDATE contract_parties SET remaining = 0, updated_at = $2
          WHERE contract_id = $1 AND remaining > 0",
        contract_id,
        now
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        "UPDATE contracts SET status = 'canceled', updated_at = $2 WHERE id = $1",
        contract_id,
        now
    )
    .execute(&mut *tx)
    .await?;

    Ok(())
}
