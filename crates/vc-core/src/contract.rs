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

use crate::page::Cursor;

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
    /// The contract fixes the receiver, and the payment names someone else —
    /// unless it names the party whose remainder it draws on, which is a return
    /// rather than a spend.
    ReceiverIsFixed,
    /// The payment names a party the contract does not name.
    NotAParty,
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
    pub currency_id: i64,
    pub application_id: i64,
    /// The public application id, used to distinguish unbound applications.
    pub client_id: String,
    /// The application's bound Bot account, not the person who registered it.
    pub bot_discord_id: Option<i64>,
    /// The application's self-chosen name, shown when it has no bound Bot.
    pub client_name: Option<String>,
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Payed {
    pub amount: i64,
    /// What the application may still spend in total: the parties' remainders
    /// summed, which is what a contract holds.
    pub remaining: i64,
    /// What the party this payment named has left, when it named one.
    ///
    /// Two different numbers, and a payment that names a party is the only thing
    /// that makes them different: `remaining` is the contract's, and this is the
    /// party's. An application billing per person reads this one — it is the
    /// answer to "how much of this subscriber's quota is left".
    pub party_remaining: Option<i64>,
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
    let named: Vec<i64> = parties.iter().map(|party| party.discord_id).collect();
    let unique: std::collections::HashSet<_> = named.iter().collect();
    if unique.len() != parties.len() {
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
        "SELECT c.id, c.currency_id, c.application_id, applications.client_id::text AS \"client_id!\",
                bot.discord_id AS bot_discord_id, applications.client_name, c.receiver_discord_id,
                c.expires_at, c.status AS \"status!\", currencies.unit, currencies.guild_id
           FROM contracts c
           JOIN currencies ON currencies.id = c.currency_id
           JOIN applications ON applications.id = c.application_id
           LEFT JOIN users bot ON bot.application_id = c.application_id
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

/// The contracts one application wrote, newest first.
///
/// `limit` absent is every one of them, which is what the endpoint answered
/// before it could be paged and what a caller that does not page still gets.
pub async fn of_application(
    pool: &PgPool,
    application_id: i64,
    cursor: Cursor,
    limit: Option<i64>,
) -> std::result::Result<Vec<Contract>, ContractError> {
    let (next, on_next) = cursors(cursor);

    let rows = sqlx::query_as!(
        ContractRow,
        "SELECT c.id, c.currency_id, c.application_id, applications.client_id::text AS \"client_id!\",
                bot.discord_id AS bot_discord_id, applications.client_name, c.receiver_discord_id,
                c.expires_at, c.status AS \"status!\", currencies.unit, currencies.guild_id
           FROM contracts c
           JOIN currencies ON currencies.id = c.currency_id
           JOIN applications ON applications.id = c.application_id
           LEFT JOIN users bot ON bot.application_id = c.application_id
          WHERE c.application_id = $1
            AND ($2::bigint IS NULL OR c.id < $2)
            AND ($3::bigint IS NULL OR c.id <= $3)
          ORDER BY c.id DESC
          LIMIT $4",
        application_id,
        next,
        on_next,
        limit
    )
    .fetch_all(pool)
    .await
    .map_err(ContractError::Database)?;

    read_all(pool, rows).await
}

/// A page of those, and where the pages around it are — the shape a screen that
/// shows five rows and four buttons needs.
///
/// One call rather than a read and a count the caller has to keep in step: the
/// total is what the screen's first line says, the page is what its buttons move
/// from, and both are asked here.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenContracts {
    pub contracts: Vec<Contract>,
    /// How many there are altogether, which is the number the screen may say.
    pub total: i64,
    /// Which page this is, counting from one, as the claim list's does.
    pub page: i64,
    /// The four places the arrows move to, each `None` when there is nowhere to go
    /// — which is what makes a button disabled rather than absent.
    pub first: Option<i64>,
    pub prev: Option<i64>,
    pub next: Option<i64>,
    pub last: Option<i64>,
}

/// One page of the contracts one user is named in that are **not over**, newest
/// first, and the count behind it.
///
/// The filter is in the statement rather than in the caller because a page that
/// could be filled by contracts which are already over would hide the ones that
/// are not: five is five rows of the thing being asked about.
pub async fn open_of_party(
    pool: &PgPool,
    discord_id: i64,
    page: i64,
    limit: i64,
) -> std::result::Result<OpenContracts, ContractError> {
    let page = page.max(1);

    let total = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM contracts c
           JOIN contract_parties p ON p.contract_id = c.id
          WHERE p.discord_id = $1 AND c.status IN ('pending', 'active')
            -- What the reader has chosen not to see: a muted currency's contracts, and the
            -- contracts a muted person is named in. The count and the page below are the same
            -- rows, and a page that hides fewer of them is an arrow that opens an empty page.
            AND NOT EXISTS (
                SELECT 1 FROM mutes mu
                 WHERE mu.user_id = (SELECT id FROM users WHERE discord_id = $1)
                   AND (mu.currency_id = c.currency_id
                        OR EXISTS (SELECT 1
                                     FROM contract_parties q
                                     JOIN users mut ON mut.id = mu.muted_user_id
                                    WHERE q.contract_id = c.id
                                      AND q.discord_id = mut.discord_id)))",
        discord_id
    )
    .fetch_one(pool)
    .await
    .map_err(ContractError::Database)?;

    let rows = sqlx::query_as!(
        ContractRow,
        "SELECT c.id, c.currency_id, c.application_id, applications.client_id::text AS \"client_id!\",
                bot.discord_id AS bot_discord_id, applications.client_name, c.receiver_discord_id,
                c.expires_at, c.status AS \"status!\", currencies.unit, currencies.guild_id
           FROM contracts c
           JOIN currencies ON currencies.id = c.currency_id
           JOIN applications ON applications.id = c.application_id
           LEFT JOIN users bot ON bot.application_id = c.application_id
           JOIN contract_parties p ON p.contract_id = c.id
          WHERE p.discord_id = $1 AND c.status IN ('pending', 'active')
            AND NOT EXISTS (
                SELECT 1 FROM mutes mu
                 WHERE mu.user_id = (SELECT id FROM users WHERE discord_id = $1)
                   AND (mu.currency_id = c.currency_id
                        OR EXISTS (SELECT 1
                                     FROM contract_parties q
                                     JOIN users mut ON mut.id = mu.muted_user_id
                                    WHERE q.contract_id = c.id
                                      AND q.discord_id = mut.discord_id)))
          ORDER BY c.id DESC
          LIMIT $2
         OFFSET $3",
        discord_id,
        limit,
        limit * (page - 1)
    )
    .fetch_all(pool)
    .await
    .map_err(ContractError::Database)?;

    let last_page = ((total + limit - 1) / limit).max(1);

    Ok(OpenContracts {
        contracts: read_all(pool, rows).await?,
        total,
        page,
        // One and n - 1, or nothing on the first page; n + 1 and the last page, or
        // nothing on the last. The claim list's arithmetic, which its arrows are
        // the same shape as.
        first: (page != 1).then_some(1),
        prev: (page != 1).then_some(page - 1),
        next: (page < last_page).then_some(page + 1),
        last: (page < last_page).then_some(last_page),
    })
}

/// The contracts one user is named in, newest first.
///
/// `limit` absent is every one of them, the same as [`of_application`].
pub async fn of_party(
    pool: &PgPool,
    discord_id: i64,
    cursor: Cursor,
    limit: Option<i64>,
) -> std::result::Result<Vec<Contract>, ContractError> {
    of_party_in(pool, discord_id, cursor, limit, &[]).await
}

/// The same list, restricted by currency before applying its cursor and limit.
/// An empty resource list means unrestricted, as it does for every grant.
pub async fn of_party_in(
    pool: &PgPool,
    discord_id: i64,
    cursor: Cursor,
    limit: Option<i64>,
    resources: &[i64],
) -> std::result::Result<Vec<Contract>, ContractError> {
    let (next, on_next) = cursors(cursor);

    let rows = sqlx::query_as!(
        ContractRow,
        "SELECT c.id, c.currency_id, c.application_id, applications.client_id::text AS \"client_id!\",
                bot.discord_id AS bot_discord_id, applications.client_name, c.receiver_discord_id,
                c.expires_at, c.status AS \"status!\", currencies.unit, currencies.guild_id
           FROM contracts c
           JOIN currencies ON currencies.id = c.currency_id
           JOIN applications ON applications.id = c.application_id
           LEFT JOIN users bot ON bot.application_id = c.application_id
           JOIN contract_parties p ON p.contract_id = c.id
          WHERE p.discord_id = $1
            AND (cardinality($5::bigint[]) = 0 OR c.currency_id = ANY($5))
            -- The reader's own mutes, as the Discord list applies them: the API answers the same
            -- person the same list, so a mute hides a contract from both or from neither.
            AND NOT EXISTS (
                SELECT 1 FROM mutes mu
                 WHERE mu.user_id = (SELECT id FROM users WHERE discord_id = $1)
                   AND (mu.currency_id = c.currency_id
                        OR EXISTS (SELECT 1
                                     FROM contract_parties q
                                     JOIN users mut ON mut.id = mu.muted_user_id
                                    WHERE q.contract_id = c.id
                                      AND q.discord_id = mut.discord_id)))
            AND ($2::bigint IS NULL OR c.id < $2)
            AND ($3::bigint IS NULL OR c.id <= $3)
          ORDER BY c.id DESC
          LIMIT $4",
        discord_id,
        next,
        on_next,
        limit,
        resources
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
    currency_id: i64,
    application_id: i64,
    client_id: String,
    bot_discord_id: Option<i64>,
    client_name: Option<String>,
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
        currency_id: row.currency_id,
        application_id: row.application_id,
        client_id: row.client_id,
        bot_discord_id: row.bot_discord_id,
        client_name: row.client_name,
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
///
/// The clock is read after acquiring the contract, party and balance locks, so
/// waiting for another transaction cannot authorize an expired contract.
pub async fn approve(
    pool: &PgPool,
    contract_id: i64,
    user_id: i32,
    clock: impl FnOnce() -> OffsetDateTime,
) -> std::result::Result<bool, ContractError> {
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

    let now = at(clock());
    if has_expired(contract.expires_at, now) {
        return Err(ContractError::Expired);
    }

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

    // The payer's account is down by the amount, so the contract's account is up
    // by it: a lock is a transfer, not a deletion, and the currency's supply is
    // the same before and after.
    //
    // The account is opened here rather than at creation because this is when a
    // contract first holds anything. `ON CONFLICT DO NOTHING` is what makes two
    // parties approving at once open one account between them.
    sqlx::query!(
        "INSERT INTO users (status, contract_id, inserted_at, updated_at)
         VALUES (NULL, $1, $2, $2)
         ON CONFLICT (contract_id) DO NOTHING",
        contract_id,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    sqlx::query!(
        "INSERT INTO assets (user_id, currency_id, amount, inserted_at, updated_at)
         SELECT u.id, $2, $3, $4, $4 FROM users u WHERE u.contract_id = $1
         ON CONFLICT (user_id, currency_id)
         DO UPDATE SET amount = assets.amount + EXCLUDED.amount,
                       updated_at = EXCLUDED.updated_at",
        contract_id,
        contract.currency_id,
        party.amount,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    // The lock is a movement, so it is written where the movement is: the party's
    // own account on the sender side and the escrow on the receiver side. A NULL
    // there rather than an account is the whole of what says this row is a lock
    // where a charge sets both sides — the escrow is not an account, so it has no
    // id to name, and a person's history can read "mine or the escrow's" out of
    // the same two columns it reads every payment's sides from.
    sqlx::query!(
        "INSERT INTO currency_payment_histories
             (amount, sender_id, receiver_id, currency_id, contract_id, \"time\", inserted_at,
              updated_at)
         VALUES ($1, $2, NULL, $3, $4, $5, $5, $5)",
        party.amount,
        i64::from(user_id),
        contract.currency_id,
        contract_id,
        now
    )
    .execute(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    sqlx::query!(
        "UPDATE contract_parties
            SET status = 'approved', remaining = amount, updated_at = $2,
                approval_order = (SELECT COALESCE(MAX(approval_order), 0) + 1
                                    FROM contract_parties WHERE contract_id = $3)
          WHERE id = $1",
        party.id,
        now,
        contract_id
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

    end(
        &mut tx,
        contract_id,
        contract.currency_id,
        Ended::Canceled,
        now,
    )
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
    clock: impl FnOnce() -> OffsetDateTime,
) -> std::result::Result<bool, ContractError> {
    let mut tx = pool.begin().await.map_err(ContractError::Database)?;

    let contract = lock_contract(&mut tx, contract_id).await?;
    let now = at(clock());

    // One that is already over has nothing left to take back: a settled expiry has
    // sent the remainders home, and a cancellation did the same.
    if contract.status != "pending" && contract.status != "active" {
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

    end(
        &mut tx,
        contract_id,
        contract.currency_id,
        Ended::Canceled,
        now,
    )
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
/// the parties in approval order and taking from each up to what the payment still
/// needs. Which party's money a payment used is not part of the contract: what
/// the parties agreed to is an amount, and FIFO is what makes every refund exact
/// without splitting anyone's remainder into fractions.
///
/// **`party_discord_id` is how a payment says whose use it bills.** Absent, the
/// draw is the FIFO one above. Present, the named party's remainder is the only
/// thing the payment may draw on, which is what a metered application wants when
/// one contract names several people: the contract is a pot, but the billing is
/// per person.
///
/// The receiver is a Discord id like the parties, and paying someone new creates
/// their account the way a payment does.
///
/// **A receiver that is one of the contract's parties is a return**, not a spend:
/// the money goes out of the escrow and into that party's own wallet, so it is
/// written the way `end` writes a return — the sender side NULL — and reads in that
/// party's own ledger like any other money coming in. It is how an application undoes
/// a use it should not have billed.
pub async fn pay(
    pool: &PgPool,
    contract_id: i64,
    application_id: i64,
    receiver_discord_id: i64,
    party_discord_id: Option<i64>,
    amount: i64,
    clock: impl FnOnce() -> OffsetDateTime,
) -> std::result::Result<Payed, ContractError> {
    let mut tx = pool.begin().await.map_err(ContractError::Database)?;

    let payed = pay_in(
        &mut tx,
        contract_id,
        application_id,
        receiver_discord_id,
        party_discord_id,
        amount,
        clock,
    )
    .await?;

    tx.commit().await.map_err(ContractError::Database)?;

    Ok(payed)
}

/// The same charge, on a transaction the caller owns — which is what lets the
/// idempotency layer's claim, the charge and the stored answer be one commit.
/// The clock is read after the contract lock is acquired, when the remaining
/// allowance and its deadline can be checked together.
pub async fn pay_in(
    tx: &mut PgConnection,
    contract_id: i64,
    application_id: i64,
    receiver_discord_id: i64,
    party_discord_id: Option<i64>,
    amount: i64,
    clock: impl FnOnce() -> OffsetDateTime,
) -> std::result::Result<Payed, ContractError> {
    if amount <= 0 {
        return Err(ContractError::InvalidAmount);
    }

    let contract = lock_contract(&mut *tx, contract_id).await?;
    let now = at(clock());

    if contract.application_id != application_id {
        return Err(ContractError::NotFound);
    }

    if contract.status == "canceled" {
        return Err(ContractError::InvalidStatus);
    }

    if has_expired(contract.expires_at, now) {
        return Err(ContractError::Expired);
    }

    // A return is not a spend. Money a party locked may always go back to that
    // party, and paying it back is the only correction an application has for a
    // use it should not have billed — so the fixed receiver gives way exactly
    // when the receiver is the party this payment draws on. What the exemption
    // is not is a way to move one party's remainder to somebody else: the party
    // has to be one the contract names, which the draw below checks.
    let returning = party_discord_id == Some(receiver_discord_id);

    if contract
        .receiver_discord_id
        .is_some_and(|fixed| fixed != receiver_discord_id)
        && !returning
    {
        return Err(ContractError::ReceiverIsFixed);
    }

    // What the application may still spend in total, which is what a payment
    // answers with whichever way it drew.
    let total = sqlx::query_scalar!(
        "SELECT COALESCE(sum(remaining), 0)::bigint AS \"remaining!\"
           FROM contract_parties WHERE contract_id = $1",
        contract_id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    // And what this payment may draw on: all of it, or the one party it named.
    // A party the contract does not name is not the same nothing as a party who
    // has already spent their part.
    let spendable = match party_discord_id {
        Some(party) => sqlx::query_scalar!(
            "SELECT remaining FROM contract_parties
              WHERE contract_id = $1 AND discord_id = $2",
            contract_id,
            party
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(ContractError::Database)?
        .ok_or(ContractError::NotAParty)?,
        None => total,
    };

    if spendable < amount {
        return Err(ContractError::NotEnoughAmount);
    }

    let taken = sqlx::query!(
        "WITH ordered AS (
             SELECT p.id, u.id AS sender_id, p.remaining,
                    COALESCE(sum(p.remaining) OVER (ORDER BY p.approval_order, p.id
                              ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0)::bigint
                        AS before
               FROM contract_parties p
               JOIN users u ON u.discord_id = p.discord_id
              WHERE p.contract_id = $1 AND p.remaining > 0
                AND ($4::bigint IS NULL OR p.discord_id = $4)
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
        now,
        party_discord_id
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    let receiver = crate::user::insert_if_not_exists(&mut *tx, receiver_discord_id)
        .await
        .map_err(ContractError::Database)?;

    sqlx::query!(
        "UPDATE assets SET amount = amount - $1, updated_at = $4
          WHERE currency_id = $2
            AND user_id = (SELECT id FROM users WHERE contract_id = $3)",
        amount,
        contract.currency_id,
        contract_id,
        now
    )
    .execute(&mut *tx)
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

    // The ledger records what moved and from whom, and **which side of the row is
    // NULL is what says what kind of movement it was** — the same pair of columns
    // every payment already used.
    //
    // When the receiver is one of the contract's parties, the money went out of the
    // escrow — which is the parties' remainders — and into that party's own wallet:
    // a return, the mirror of the lock their approval wrote, and a movement of the
    // person's money rather than of an application's. Charge-shaped, with both sides
    // set, it said the opposite, and a wallet history that reads the NULL side was
    // right to leave it out.
    //
    // The contract's own names are what answer this, rather than the request: a
    // payment that names no party still lands in one when the receiver is one of them.
    let receiver_is_party = sqlx::query_scalar!(
        "SELECT EXISTS (SELECT 1 FROM contract_parties
                         WHERE contract_id = $1 AND discord_id = $2) AS \"exists!\"",
        contract_id,
        receiver_discord_id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(ContractError::Database)?;

    if receiver_is_party {
        // One row for the whole movement rather than one per drawn remainder: the
        // sender side is the escrow, which is nobody's account, so which slice came
        // from which party is not a thing this row could say. The amount is what the
        // wallet received.
        sqlx::query!(
            "INSERT INTO currency_payment_histories
                 (amount, sender_id, receiver_id, currency_id, contract_id, \"time\", inserted_at,
                  updated_at)
             VALUES ($1, NULL, $2, $3, $4, $5, $5, $5)",
            amount,
            i64::from(receiver.id),
            contract.currency_id,
            contract_id,
            now
        )
        .execute(&mut *tx)
        .await
        .map_err(ContractError::Database)?;
    } else {
        // Otherwise one row per party whose remainder was drawn on, which is the
        // money's own path into the receiver's balance rather than a note that an
        // application paid. The contract is named beside it, because a statement per
        // contract cannot be read out of rows that do not say which contract they
        // belong to.
        let amounts: Vec<i64> = taken.iter().map(|row| row.amount).collect();
        let senders: Vec<i64> = taken.iter().map(|row| i64::from(row.sender_id)).collect();

        // `ORDER BY p.approval_order, p.id` is what makes a payment's rows ordered rather than
        // unfortunate: without it the rows of one payment are inserted in whatever
        // order the draw returned them, and a statement lists them by id. This is the
        // draw's own order — oldest approval first — so a reader sees the slices in
        // the order the money left.
        sqlx::query!(
            "INSERT INTO currency_payment_histories
                 (amount, sender_id, receiver_id, currency_id, contract_id, \"time\", inserted_at,
                  updated_at)
             SELECT t.amount, t.sender_id, $3, $4, $5, $6, $6, $6
               FROM UNNEST($1::bigint[], $2::bigint[]) AS t(amount, sender_id)
               JOIN users u ON u.id = t.sender_id
               JOIN contract_parties p ON p.contract_id = $5 AND p.discord_id = u.discord_id
              ORDER BY p.approval_order, p.id",
            &amounts,
            &senders,
            i64::from(receiver.id),
            contract.currency_id,
            contract_id,
            now
        )
        .execute(&mut *tx)
        .await
        .map_err(ContractError::Database)?;
    }

    Ok(Payed {
        amount,
        remaining: total - amount,
        // What the named party had, less what this payment took from them: the
        // draw above took exactly the amount out of their remainder, whether it
        // was drawn from all of them or from the one named.
        party_remaining: party_discord_id.map(|_| spendable - amount),
    })
}

/// One row of a contract's statement, in the shape it names it: which movement it
/// was, which party's remainder it drew on, how much, where it went, and when.
///
/// One API payment writes **one row per party drawn on** — a payment that names
/// no party draws oldest-approval-first across as many as it needs — so a list of
/// these is a list of ledger rows rather than of charges. For the one-party
/// contract a metered application writes, the two are the same list.
///
/// A statement now holds three kinds of row in one table, and the kind is the
/// `event`: a **lock** is a party's approval (their account on the sender side,
/// the escrow — no account — on the receiver side), a **return** is the escrow
/// sending a remainder home (the mirror of the lock, whether `end` did it or a
/// payment whose receiver was the party), and a **charge** is the application
/// spending what was locked (both sides set). The NULL side is what tells them
/// apart, so no column was added for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Payment {
    /// The ledger row, which is what a page resumes from and what a reader can
    /// point at.
    pub id: i64,
    /// `lock` when the row is a party's approval, `return` when it is money going
    /// home, `charge` when it is the application spending. Computed from which
    /// side of the row is NULL.
    pub event: &'static str,
    /// The party the money came out of — the account the row names is theirs, and
    /// this is the Discord id it belongs to. `None` on a return, which has no
    /// sender: the escrow sent it.
    pub discord_id: Option<i64>,
    pub amount: i64,
    /// The party the money went to, by Discord id. `None` on a lock, which has no
    /// receiver: the escrow received it.
    pub receiver_discord_id: Option<i64>,
    pub time: PrimitiveDateTime,
}

/// The payments made under one contract, newest first.
///
/// The cursor is the ledger's own id: rows are only ever appended, so it orders
/// them the way the list reads and pages without a timestamp to collide on — and
/// `(contract_id, id)` is exactly the index this query wants.
///
/// Every row here is one of the three contract movements `approve`, `pay` and
/// `end` write, and each leaves one side of the row NULL where it has no account
/// to name — the escrow, which is not a user — so both joins are outer and both
/// Discord ids are nullable. The schema's nullability is the Elixir's besides:
/// it writes rows this never selects (`amount`, `time`).
pub async fn payments(
    pool: &PgPool,
    contract_id: i64,
    cursor: Cursor,
    limit: Option<i64>,
) -> std::result::Result<Vec<Payment>, ContractError> {
    let (next, on_next) = cursors(cursor);

    let rows = sqlx::query_as!(
        PaymentRow,
        "SELECT history.id, sender.discord_id AS discord_id, history.amount AS \"amount!\",
                receiver.discord_id AS receiver_discord_id, history.\"time\" AS \"time!\",
                history.sender_id IS NULL AS \"sender_unset!\",
                history.receiver_id IS NULL AS \"receiver_unset!\"
           FROM currency_payment_histories history
           LEFT JOIN users sender ON sender.id = history.sender_id
           LEFT JOIN users receiver ON receiver.id = history.receiver_id
          WHERE history.contract_id = $1
            AND ($2::bigint IS NULL OR history.id < $2)
            AND ($3::bigint IS NULL OR history.id <= $3)
          ORDER BY history.id DESC
          LIMIT $4",
        contract_id,
        next,
        on_next,
        limit
    )
    .fetch_all(pool)
    .await
    .map_err(ContractError::Database)?;

    Ok(rows
        .into_iter()
        .map(|row| Payment {
            id: row.id,
            event: event(row.sender_unset, row.receiver_unset),
            discord_id: row.discord_id,
            amount: row.amount,
            receiver_discord_id: row.receiver_discord_id,
            time: row.time,
        })
        .collect())
}

struct PaymentRow {
    id: i64,
    discord_id: Option<i64>,
    amount: i64,
    receiver_discord_id: Option<i64>,
    time: PrimitiveDateTime,
    sender_unset: bool,
    receiver_unset: bool,
}

/// Which movement a statement row is, out of which side of it is NULL: a lock has
/// no receiver, a return has no sender, and a charge — both sides set — is what
/// is left.
fn event(sender_unset: bool, receiver_unset: bool) -> &'static str {
    match (sender_unset, receiver_unset) {
        (false, true) => "lock",
        (true, false) => "return",
        _ => "charge",
    }
}

/// The two comparisons a cursor may be, as the pair the list queries bind: `next`
/// is exclusive, `on_next` is inclusive, and at most one of them is ever set.
fn cursors(cursor: Cursor) -> (Option<i64>, Option<i64>) {
    match cursor {
        Cursor::Next(value) => (Some(value), None),
        Cursor::OnNext(value) => (None, Some(value)),
        Cursor::First => (None, None),
    }
}

fn has_expired(expires_at: Option<PrimitiveDateTime>, now: PrimitiveDateTime) -> bool {
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

/// Why a contract is over, which is what its status records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// A party refused it, or took their part back.
    Canceled,
    /// Its deadline passed with it still standing.
    Expired,
}

impl Ended {
    fn status(self) -> &'static str {
        match self {
            Ended::Canceled => "canceled",
            Ended::Expired => "expired",
        }
    }
}

/// The end of a contract: what is left of every party's lock goes back to them,
/// and the contract joins the ones that are over, for the reason given.
///
/// One statement per thing rather than one per party: the refund is an upsert
/// over however many parties still hold something, so a contract with fifty of
/// them costs what one with one costs.
async fn end(
    tx: &mut PgConnection,
    contract_id: i64,
    currency_id: i64,
    ended: Ended,
    now: PrimitiveDateTime,
) -> std::result::Result<(), sqlx::Error> {
    // The contract row already serializes access to its escrow. Refunds also
    // touch several public wallets, so lock those accounts in the same order as
    // ordinary transfers before writing balances, including absent asset rows.
    let recipients = sqlx::query_scalar!(
        "SELECT u.id
           FROM contract_parties p
           JOIN users u ON u.discord_id = p.discord_id
          WHERE p.contract_id = $1 AND p.remaining > 0",
        contract_id
    )
    .fetch_all(&mut *tx)
    .await?;
    crate::transfer::lock_participants(&mut *tx, &recipients).await?;

    sqlx::query!(
        "UPDATE assets
            SET amount = amount - (SELECT COALESCE(SUM(p.remaining), 0)
                                     FROM contract_parties p
                                    WHERE p.contract_id = $1),
                updated_at = $3
          WHERE currency_id = $2
            AND user_id = (SELECT id FROM users WHERE contract_id = $1)",
        contract_id,
        currency_id,
        now
    )
    .execute(&mut *tx)
    .await?;

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

    // And the ledger says the same thing the balances just did: one row per party
    // whose remainder went home, the escrow on the sender side this time — a NULL
    // there is what marks a return where a charge sets both sides, the mirror of
    // the lock the approval wrote. A party with nothing left is not a movement
    // and writes no row, as the refund above writes no balance for them.
    sqlx::query!(
        "INSERT INTO currency_payment_histories
             (amount, sender_id, receiver_id, currency_id, contract_id, \"time\", inserted_at,
              updated_at)
         SELECT p.remaining, NULL, u.id, $2, $1, $3, $3, $3
           FROM contract_parties p
           JOIN users u ON u.discord_id = p.discord_id
          WHERE p.contract_id = $1 AND p.remaining > 0",
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
        "UPDATE contracts SET status = $2, updated_at = $3 WHERE id = $1",
        contract_id,
        ended.status(),
        now
    )
    .execute(&mut *tx)
    .await?;

    Ok(())
}

/// A chunk of the contracts whose deadline has passed and that nobody has settled
/// yet: the ones that ran out longest ago first, and at most `limit` of them.
///
/// What the clock asks for: a contract still standing — `pending` or `active` —
/// whose time is up. The ones already over are not looked at again, which is what
/// the partial index behind this query is for.
///
/// **The order and the bound are one decision, and they are the index's.**
/// `contracts_unsettled_expiry_index` is keyed on `expires_at`, so it can serve the
/// filter *and* an order on the same column — deadline order — and a scan of it can
/// then stop the moment it has `limit` rows. Ordered by `id` instead, the index
/// serves only the filter: every matching row is read, fetched from the heap and
/// sorted before the first one comes back, so the pass costs the whole backlog and
/// grows with it. Reading the deadline order costs the chunk.
///
/// **Deadline order is also the order to settle in.** Every row is a contract whose
/// time is up and whose money is locked, so the one that ran out first is the one
/// to refund first; among contracts created with a deadline after them, deadlines
/// and ids rise together anyway. Ties are not broken: a second sort key would make
/// the executor sort every row that shares the first one, which is what makes the
/// stop-early scan stop early no longer. Progress needs no tiebreak, because a
/// later deadline is always a later candidate — a contract created now is due no
/// earlier than the next tick — so each tick takes the next `limit` rows off the
/// front of the same order and the rest are still there next tick.
pub async fn expired(
    pool: &PgPool,
    now: OffsetDateTime,
    limit: i64,
) -> std::result::Result<Vec<i64>, ContractError> {
    let ids = sqlx::query_scalar!(
        "SELECT id FROM contracts
          WHERE status IN ('pending', 'active')
            AND expires_at IS NOT NULL AND expires_at <= $1
          ORDER BY expires_at
          LIMIT $2",
        at(now),
        limit
    )
    .fetch_all(pool)
    .await
    .map_err(ContractError::Database)?;

    Ok(ids)
}

/// The end of a contract that ran out of time: what is left of every party's lock
/// goes home, and the contract is marked `expired` rather than `canceled` — the
/// deadline did it, not anyone in it.
///
/// Answers whether it did anything, which is what tells the caller to say so.
/// Idempotent and safe against a race with a party: the contract row is locked
/// first, so a withdrawal and a settlement cannot both move the same remainder,
/// and the second of the two finds a contract that is already over.
/// Lock waits are bounded for this background operation, including locks on
/// refund recipients. A busy contract rolls back and is retried on a later tick.
pub async fn settle(
    pool: &PgPool,
    contract_id: i64,
    now: OffsetDateTime,
) -> std::result::Result<bool, ContractError> {
    let now = at(now);
    let mut tx = pool.begin().await.map_err(ContractError::Database)?;

    sqlx::query("SET LOCAL lock_timeout = '100ms'")
        .execute(&mut *tx)
        .await
        .map_err(ContractError::Database)?;

    let contract = lock_contract(&mut tx, contract_id).await?;

    if contract.status != "pending" && contract.status != "active" {
        return Ok(false);
    }

    if !has_expired(contract.expires_at, now) {
        return Ok(false);
    }

    end(
        &mut tx,
        contract_id,
        contract.currency_id,
        Ended::Expired,
        now,
    )
    .await
    .map_err(ContractError::Database)?;

    tx.commit().await.map_err(ContractError::Database)?;

    Ok(true)
}
