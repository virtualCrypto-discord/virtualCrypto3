use std::collections::HashMap;

use serde_json::{Value, json};
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool};
use time::PrimitiveDateTime;

use crate::error::Result;
use crate::model::utc_now;
use crate::notification::Notifier;
use crate::transfer::TransferError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimUser {
    pub id: i32,
    pub discord_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCurrency {
    pub name: Option<String>,
    pub unit: Option<String>,
    pub guild_id: Option<i64>,
    pub pool_amount: Option<i64>,
}

/// The shape `format_claim/2` serializes: the claim joined to its currency and to
/// both users, plus the requesting user's own metadata (or `{}`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimView {
    pub id: i64,
    pub amount: Option<i64>,
    pub status: Option<String>,
    pub inserted_at: PrimitiveDateTime,
    pub updated_at: PrimitiveDateTime,
    pub currency: ClaimCurrency,
    pub claimant: ClaimUser,
    pub payer: ClaimUser,
    pub metadata: Value,
}

struct Row {
    claim_id: i64,
    claim_amount: Option<i64>,
    claim_status: Option<String>,
    claim_inserted_at: PrimitiveDateTime,
    claim_updated_at: PrimitiveDateTime,
    currency_name: Option<String>,
    currency_unit: Option<String>,
    currency_guild_id: Option<i64>,
    currency_pool_amount: Option<i64>,
    claimant_id: i32,
    claimant_discord_id: Option<i64>,
    payer_id: i32,
    payer_discord_id: Option<i64>,
    metadata: Json<Value>,
}

impl Row {
    fn into_view(self) -> ClaimView {
        ClaimView {
            id: self.claim_id,
            amount: self.claim_amount,
            status: self.claim_status,
            inserted_at: self.claim_inserted_at,
            updated_at: self.claim_updated_at,
            currency: ClaimCurrency {
                name: self.currency_name,
                unit: self.currency_unit,
                guild_id: self.currency_guild_id,
                pool_amount: self.currency_pool_amount,
            },
            claimant: ClaimUser {
                id: self.claimant_id,
                discord_id: self.claimant_discord_id,
            },
            payer: ClaimUser {
                id: self.payer_id,
                discord_id: self.payer_discord_id,
            },
            metadata: self.metadata.0,
        }
    }
}

/// Mirrors `Query.Claim.get_claim_by_id/2` (the two-argument clause that also
/// loads the executor's metadata).
///
/// The metadata comes from a `LEFT JOIN` on `claim_metadata` restricted to the
/// executing user, wrapped in `COALESCE(..., '{}')`, so a claim with no metadata
/// row for that user still yields an empty object.
pub async fn view(pool: &PgPool, operator_id: i32, claim_id: i64) -> Result<Option<ClaimView>> {
    let row = sqlx::query_as!(
        Row,
        "SELECT c.id AS \"claim_id!\",
                c.amount AS claim_amount,
                c.status::text AS claim_status,
                c.inserted_at AS \"claim_inserted_at!\",
                c.updated_at AS \"claim_updated_at!\",
                cur.name AS currency_name,
                cur.unit AS currency_unit,
                cur.guild_id AS currency_guild_id,
                cur.pool_amount AS currency_pool_amount,
                cl.id AS \"claimant_id!\",
                cl.discord_id AS claimant_discord_id,
                py.id AS \"payer_id!\",
                py.discord_id AS payer_discord_id,
                COALESCE(m.metadata, '{}'::jsonb) AS \"metadata!\"
           FROM claims c
           JOIN currencies cur ON c.currency_id = cur.id
           JOIN users cl ON c.claimant_user_id = cl.id
           JOIN users py ON c.payer_user_id = py.id
           LEFT JOIN claim_metadata m
                  ON c.id = m.claim_id AND m.owner_user_id = $1
          WHERE c.id = $2",
        i64::from(operator_id),
        claim_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(Row::into_view))
}

/// `Query.Claim.get_claim_by_ids/2`: the same view as [`view`] for several ids,
/// returned in the order they were asked for, which is the order the caller
/// packed them in.
pub async fn views_by_ids(pool: &PgPool, operator_id: i32, ids: &[i64]) -> Result<Vec<ClaimView>> {
    let rows = sqlx::query_as!(
        Row,
        "SELECT c.id AS \"claim_id!\",
                c.amount AS claim_amount,
                c.status::text AS claim_status,
                c.inserted_at AS \"claim_inserted_at!\",
                c.updated_at AS \"claim_updated_at!\",
                cur.name AS currency_name,
                cur.unit AS currency_unit,
                cur.guild_id AS currency_guild_id,
                cur.pool_amount AS currency_pool_amount,
                cl.id AS \"claimant_id!\",
                cl.discord_id AS claimant_discord_id,
                py.id AS \"payer_id!\",
                py.discord_id AS payer_discord_id,
                COALESCE(m.metadata, '{}'::jsonb) AS \"metadata!\"
           FROM claims c
           JOIN currencies cur ON c.currency_id = cur.id
           JOIN users cl ON c.claimant_user_id = cl.id
           JOIN users py ON c.payer_user_id = py.id
           LEFT JOIN claim_metadata m
                  ON c.id = m.claim_id AND m.owner_user_id = $1
          WHERE c.id = ANY($2)",
        i64::from(operator_id),
        ids
    )
    .fetch_all(pool)
    .await?;

    let mut views: Vec<ClaimView> = rows.into_iter().map(Row::into_view).collect();
    views.sort_by_key(|view| {
        ids.iter()
            .position(|id| *id == view.id)
            .unwrap_or(usize::MAX)
    });

    Ok(views)
}

/// Which side of a claim the operating user must be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrFilter {
    All,
    Received,
    Claimed,
}

impl SrFilter {
    fn as_str(self) -> &'static str {
        match self {
            SrFilter::All => "all",
            SrFilter::Received => "received",
            SrFilter::Claimed => "claimed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Desc,
    Asc,
}

impl Order {
    fn as_str(self) -> &'static str {
        match self {
            Order::Desc => "desc",
            Order::Asc => "asc",
        }
    }
}

/// Where to resume from. This list was the first to be paged and the shape it
/// used is the one every other paged list here uses, so the type lives in
/// `crate::page` now and this is where callers still find it.
pub use crate::page::Cursor;

pub struct ClaimFilter<'a> {
    pub operator_id: i32,
    pub statuses: &'a [String],
    pub sr_filter: SrFilter,
    pub related_user_id: Option<i64>,
    pub order: Order,
    pub cursor: Cursor,
    pub limit: Option<i64>,
}

/// Mirrors `Raw.Get.get_claims/7` through `Query.Claim.get_claims/7`.
///
/// The status filter, side filter, related-user filter, cursor and limit are all
/// expressed as parameters of one statement so the query stays compile-checked.
/// `LIMIT NULL` means no limit, which is exactly what Elixir does when no limit
/// parameter is supplied.
pub async fn list(pool: &PgPool, filter: ClaimFilter<'_>) -> Result<Vec<ClaimView>> {
    let rows = sqlx::query_as!(
        Row,
        "SELECT c.id AS \"claim_id!\",
                c.amount AS claim_amount,
                c.status::text AS claim_status,
                c.inserted_at AS \"claim_inserted_at!\",
                c.updated_at AS \"claim_updated_at!\",
                cur.name AS currency_name,
                cur.unit AS currency_unit,
                cur.guild_id AS currency_guild_id,
                cur.pool_amount AS currency_pool_amount,
                cl.id AS \"claimant_id!\",
                cl.discord_id AS claimant_discord_id,
                py.id AS \"payer_id!\",
                py.discord_id AS payer_discord_id,
                COALESCE(m.metadata, '{}'::jsonb) AS \"metadata!\"
           FROM claims c
           JOIN currencies cur ON c.currency_id = cur.id
           JOIN users cl ON c.claimant_user_id = cl.id
           JOIN users py ON c.payer_user_id = py.id
           LEFT JOIN claim_metadata m
                  ON c.id = m.claim_id AND m.owner_user_id = $4
          WHERE c.status::text = ANY($1)
            AND (($2 = 'all' AND (c.payer_user_id = $4 OR c.claimant_user_id = $4))
              OR ($2 = 'received' AND c.payer_user_id = $4)
              OR ($2 = 'claimed' AND c.claimant_user_id = $4))
            AND ($3::bigint IS NULL
                 OR c.payer_user_id = $3 OR c.claimant_user_id = $3)
            -- What the reader has chosen not to see: a muted currency's claims, and the claims
            -- of a muted person. In the statement rather than in the caller because the count
            -- and the page are the same rows, and a page that leaves out fewer of them than the
            -- count does is an arrow that opens an empty page.
            AND NOT EXISTS (
                SELECT 1 FROM mutes mu
                 WHERE mu.user_id = $4
                   AND (mu.currency_id = c.currency_id
                        OR mu.muted_user_id = c.claimant_user_id
                        OR mu.muted_user_id = c.payer_user_id))
            AND ($6 = 'first'
                 OR ($7 = 'desc' AND $6 = 'next' AND c.id < COALESCE($5::bigint, 0))
                 OR ($7 = 'desc' AND $6 = 'on_next' AND c.id <= COALESCE($5::bigint, 0))
                 OR ($7 = 'asc' AND $6 = 'next' AND c.id > COALESCE($5::bigint, 0))
                 OR ($7 = 'asc' AND $6 = 'on_next' AND c.id >= COALESCE($5::bigint, 0)))
          ORDER BY CASE WHEN $7 = 'desc' THEN c.id END DESC,
                   CASE WHEN $7 = 'asc' THEN c.id END ASC
          LIMIT $8::bigint",
        filter.statuses,
        filter.sr_filter.as_str(),
        filter.related_user_id,
        i64::from(filter.operator_id),
        filter.cursor.value(),
        filter.cursor.kind(),
        filter.order.as_str(),
        filter.limit
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Row::into_view).collect())
}

/// The number of the last page, which is what `:last` asks for.
pub async fn last_page_number(
    pool: &PgPool,
    operator_id: i32,
    statuses: &[String],
    sr_filter: SrFilter,
    related_user_id: Option<i64>,
    limit: i64,
) -> Result<i64> {
    let total = sqlx::query_scalar!(
        "SELECT count(c.id) AS \"total!\"
           FROM claims c
          WHERE c.status::text = ANY($1)
            AND (($2 = 'all' AND (c.payer_user_id = $3 OR c.claimant_user_id = $3))
              OR ($2 = 'received' AND c.payer_user_id = $3)
              OR ($2 = 'claimed' AND c.claimant_user_id = $3))
            AND ($4::bigint IS NULL
                 OR c.payer_user_id = $4 OR c.claimant_user_id = $4)
            -- The page's own filter, so that the last page is the last page of what the reader
            -- can see rather than of what the table holds.
            AND NOT EXISTS (
                SELECT 1 FROM mutes mu
                 WHERE mu.user_id = $3
                   AND (mu.currency_id = c.currency_id
                        OR mu.muted_user_id = c.claimant_user_id
                        OR mu.muted_user_id = c.payer_user_id))",
        statuses,
        sr_filter.as_str(),
        i64::from(operator_id),
        related_user_id
    )
    .fetch_one(pool)
    .await?;

    Ok(((total + limit - 1) / limit).max(1))
}

/// Where a pagination button moves to. `:last` cannot be expressed as a number,
/// so the payload carries a page of zero for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTarget {
    Last,
    Number(i64),
}

/// A page of the claim list, and the pages its buttons move to.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimPage {
    pub claims: Vec<ClaimView>,
    pub first: Option<i64>,
    pub prev: Option<i64>,
    pub next: Option<PageTarget>,
    pub last: Option<PageTarget>,
    pub page: i64,
}

/// `Raw.Get.get_claims/7` for `%{page: n}`: one more row than the page holds is
/// fetched, which is how the caller learns whether another page follows.
pub async fn list_page(
    pool: &PgPool,
    operator_id: i32,
    statuses: &[String],
    sr_filter: SrFilter,
    related_user_id: Option<i64>,
    page: i64,
    limit: i64,
) -> Result<ClaimPage> {
    let rows = sqlx::query_as!(
        Row,
        "SELECT c.id AS \"claim_id!\",
                c.amount AS claim_amount,
                c.status::text AS claim_status,
                c.inserted_at AS \"claim_inserted_at!\",
                c.updated_at AS \"claim_updated_at!\",
                cur.name AS currency_name,
                cur.unit AS currency_unit,
                cur.guild_id AS currency_guild_id,
                cur.pool_amount AS currency_pool_amount,
                cl.id AS \"claimant_id!\",
                cl.discord_id AS claimant_discord_id,
                py.id AS \"payer_id!\",
                py.discord_id AS payer_discord_id,
                COALESCE(m.metadata, '{}'::jsonb) AS \"metadata!\"
           FROM claims c
           JOIN currencies cur ON c.currency_id = cur.id
           JOIN users cl ON c.claimant_user_id = cl.id
           JOIN users py ON c.payer_user_id = py.id
           LEFT JOIN claim_metadata m
                  ON c.id = m.claim_id AND m.owner_user_id = $4
          WHERE c.status::text = ANY($1)
            AND (($2 = 'all' AND (c.payer_user_id = $4 OR c.claimant_user_id = $4))
              OR ($2 = 'received' AND c.payer_user_id = $4)
              OR ($2 = 'claimed' AND c.claimant_user_id = $4))
            AND ($3::bigint IS NULL
                 OR c.payer_user_id = $3 OR c.claimant_user_id = $3)
            -- The same filter the count applies: one more row than the page holds is how the
            -- caller learns whether another page follows, so a hidden row counted here would be
            -- an arrow leading to an empty page.
            AND NOT EXISTS (
                SELECT 1 FROM mutes mu
                 WHERE mu.user_id = $4
                   AND (mu.currency_id = c.currency_id
                        OR mu.muted_user_id = c.claimant_user_id
                        OR mu.muted_user_id = c.payer_user_id))
          ORDER BY c.id DESC
          LIMIT $5
         OFFSET $6",
        statuses,
        sr_filter.as_str(),
        related_user_id,
        i64::from(operator_id),
        limit + 1,
        limit * (page - 1)
    )
    .fetch_all(pool)
    .await?;

    let more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > limit;
    let claims: Vec<ClaimView> = rows
        .into_iter()
        .take(limit as usize)
        .map(Row::into_view)
        .collect();

    Ok(ClaimPage {
        claims,
        // `{first, prev}` are 1 and n - 1, or nothing on the first page.
        first: if page != 1 { Some(1) } else { None },
        prev: if page != 1 { Some(page - 1) } else { None },
        // `{last, next}` are `:last` and n + 1, or nothing on the last page.
        last: if more { Some(PageTarget::Last) } else { None },
        next: if more {
            Some(PageTarget::Number(page + 1))
        } else {
            None
        },
        page,
    })
}

/// `Query.Claim.create_claim/5` failures.
#[derive(Debug)]
pub enum CreateError {
    InvalidAmount,
    NotFoundCurrency,
    Database(sqlx::Error),
}

/// `Query.Claim.create_claim/5`: resolve (or create) the payer, insert a pending
/// claim, and attach the claimant's metadata when one was supplied.
///
/// The claimant's metadata row is owned by the claimant, which is what makes the
/// metadata private to each side. A failed amount guard is reported before any
/// validation happens, matching the Elixir function's guard.
pub async fn create(
    pool: &PgPool,
    claimant_id: i32,
    payer_discord_id: i64,
    unit: &str,
    amount: i64,
    metadata: Option<Value>,
) -> std::result::Result<i64, CreateError> {
    let mut tx = pool.begin().await.map_err(CreateError::Database)?;
    let claim_id = create_in(
        &mut tx,
        claimant_id,
        payer_discord_id,
        unit,
        amount,
        metadata,
    )
    .await?;
    tx.commit().await.map_err(CreateError::Database)?;
    Ok(claim_id)
}

/// Create on the caller's transaction, so a resource authorization lock can
/// cover both the unit lookup and the insert.
pub async fn create_in(
    tx: &mut PgConnection,
    claimant_id: i32,
    payer_discord_id: i64,
    unit: &str,
    amount: i64,
    metadata: Option<Value>,
) -> std::result::Result<i64, CreateError> {
    if amount <= 0 {
        return Err(CreateError::InvalidAmount);
    }

    let currency_id = sqlx::query_scalar!("SELECT id FROM currencies WHERE unit = $1", unit)
        .fetch_optional(&mut *tx)
        .await
        .map_err(CreateError::Database)?
        .ok_or(CreateError::NotFoundCurrency)?;

    let payer = crate::user::insert_if_not_exists(&mut *tx, payer_discord_id)
        .await
        .map_err(CreateError::Database)?;

    let now = utc_now();
    let claim_id = sqlx::query_scalar!(
        "INSERT INTO claims
             (amount, status, claimant_user_id, payer_user_id, currency_id, inserted_at, updated_at)
         VALUES ($1, 'pending'::text::virtual_crypto_claim_status, $2, $3, $4, $5, $5)
         RETURNING id",
        amount,
        i64::from(claimant_id),
        i64::from(payer.id),
        currency_id,
        now
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(CreateError::Database)?;

    if let Some(metadata) = metadata {
        sqlx::query!(
            "INSERT INTO claim_metadata
                 (claim_id, claimant_user_id, payer_user_id, owner_user_id, metadata)
             VALUES ($1, $2, $3, $4, $5)",
            claim_id,
            i64::from(claimant_id),
            i64::from(payer.id),
            i64::from(claimant_id),
            metadata
        )
        .execute(&mut *tx)
        .await
        .map_err(CreateError::Database)?;
    }

    Ok(claim_id)
}

/// The status transition a `PATCH` performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Approved,
    Denied,
    Canceled,
}

impl Transition {
    fn status(self) -> &'static str {
        match self {
            Transition::Approved => "approved",
            Transition::Denied => "denied",
            Transition::Canceled => "canceled",
        }
    }

    /// `approve_claim/3` and `deny_claim/3` require the payer as operator;
    /// `cancel_claim/3` requires the claimant.
    fn requires_payer(self) -> bool {
        matches!(self, Transition::Approved | Transition::Denied)
    }
}

#[derive(Debug)]
pub enum TransitionError {
    NotFound,
    InvalidStatus,
    InvalidOperator,
    /// A non-positive amount reached the transfer, which a stored claim cannot
    /// have; reachable only through a corrupted row.
    InvalidAmount,
    NotEnoughAmount,
    NotFoundCurrency,
    NotFoundSenderAsset,
    /// The `metadata_limitation` trigger rejected the write. The controller
    /// answers this with its long "upper limit" message and no details.
    MetadataLimit,
    Database(sqlx::Error),
}

impl From<TransferError> for TransitionError {
    fn from(error: TransferError) -> Self {
        match error {
            TransferError::InvalidAmount => TransitionError::InvalidAmount,
            TransferError::NotFoundCurrency => TransitionError::NotFoundCurrency,
            TransferError::NotFoundSenderAsset => TransitionError::NotFoundSenderAsset,
            TransferError::NotEnoughAmount => TransitionError::NotEnoughAmount,
            TransferError::Database(error) => TransitionError::Database(error),
        }
    }
}

/// What `get_claim_by_id_with_lock/1` selects: enough to validate the operator
/// and move the money, without the metadata join.
#[derive(Debug, Clone)]
pub struct LockedClaim {
    pub status: Option<String>,
    pub amount: Option<i64>,
    pub currency_unit: Option<String>,
    pub claimant_id: i64,
    pub payer_id: i64,
}

async fn lock(
    conn: &mut PgConnection,
    claim_id: i64,
) -> std::result::Result<Option<LockedClaim>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT c.status::text AS status,
                c.amount AS amount,
                cur.unit AS currency_unit,
                c.claimant_user_id AS \"claimant_id!\",
                c.payer_user_id AS \"payer_id!\"
           FROM claims c
           JOIN currencies cur ON c.currency_id = cur.id
          WHERE c.id = $1
            FOR UPDATE OF c",
        claim_id
    )
    .fetch_optional(&mut *conn)
    .await?;

    Ok(row.map(|row| LockedClaim {
        status: row.status,
        amount: row.amount,
        currency_unit: row.currency_unit,
        claimant_id: row.claimant_id,
        payer_id: row.payer_id,
    }))
}

/// `Query.Claim.update_claim_status/4`: compare-and-set on `status = 'pending'`,
/// then either upsert the operator's metadata or delete it.
async fn update_status(
    conn: &mut PgConnection,
    operator_id: i32,
    claim_id: i64,
    status: &str,
    metadata: Option<Value>,
    claimant_id: i64,
    payer_id: i64,
) -> std::result::Result<(), TransitionError> {
    let now = utc_now();

    let updated = sqlx::query!(
        "UPDATE claims
            SET status = $2::text::virtual_crypto_claim_status, updated_at = $3
          WHERE id = $1 AND status = 'pending'
          RETURNING id",
        claim_id,
        status,
        now
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(TransitionError::Database)?;

    if updated.is_none() {
        return Err(TransitionError::NotFound);
    }

    let owner_id = i64::from(operator_id);

    match metadata {
        Some(metadata) => {
            upsert_metadata(conn, claim_id, claimant_id, payer_id, owner_id, metadata).await
        }
        None => {
            delete_metadata(conn, claim_id, owner_id).await?;
            Ok(())
        }
    }
}

/// `ON CONFLICT ... DO UPDATE SET metadata = jsonb_strip_nulls(existing || new)`.
///
/// A `check_violation` is reported as [`TransitionError::MetadataLimit`], which is
/// what Elixir's rescue does — note that this also catches the object-shape check.
async fn upsert_metadata(
    conn: &mut PgConnection,
    claim_id: i64,
    claimant_id: i64,
    payer_id: i64,
    owner_id: i64,
    metadata: Value,
) -> std::result::Result<(), TransitionError> {
    let result = sqlx::query!(
        "INSERT INTO claim_metadata
             (claim_id, claimant_user_id, payer_user_id, owner_user_id, metadata)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (claim_id, owner_user_id)
         DO UPDATE SET metadata = jsonb_strip_nulls(claim_metadata.metadata || EXCLUDED.metadata)",
        claim_id,
        claimant_id,
        payer_id,
        owner_id,
        metadata
    )
    .execute(&mut *conn)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(error) => match error.as_database_error().and_then(|error| error.code()) {
            Some(code) if code == "23514" => Err(TransitionError::MetadataLimit),
            _ => Err(TransitionError::Database(error)),
        },
    }
}

async fn delete_metadata(
    conn: &mut PgConnection,
    claim_id: i64,
    owner_id: i64,
) -> std::result::Result<(), TransitionError> {
    sqlx::query!(
        "DELETE FROM claim_metadata WHERE claim_id = $1 AND owner_user_id = $2",
        claim_id,
        owner_id
    )
    .execute(&mut *conn)
    .await
    .map_err(TransitionError::Database)?;

    Ok(())
}

/// `Money.format_claim_for_notification/1`: the event an application receives
/// about a claim of one of its users, together with the claimant it belongs to.
///
/// The metadata is the claimant's own row, which is what `approve_claim/3`
/// passes after taking it off the transition result — never the operator's.
async fn claim_notification(
    conn: &mut PgConnection,
    claim_id: i64,
    claimant_id: i64,
) -> std::result::Result<Option<(i32, Value)>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT c.id, c.amount, c.status::text AS status, c.updated_at,
                cur.id AS \"currency_id!\", cur.unit, cur.name,
                cur.guild_id, cur.pool_amount,
                payer.id AS \"payer_id!\", payer.discord_id,
                claimant.id AS \"claimant_id!\"
           FROM claims c
           JOIN currencies cur ON cur.id = c.currency_id
           JOIN users payer ON payer.id = c.payer_user_id
           JOIN users claimant ON claimant.id = c.claimant_user_id
          WHERE c.id = $1",
        claim_id
    )
    .fetch_optional(&mut *conn)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    // `format_claim_for_notification/1` only describes the two statuses it
    // announces; its `case` has no clause for anything else.
    let status = match row.status.as_deref() {
        Some("approved") => "approved",
        Some("denied") => "denied",
        _ => return Ok(None),
    };

    let payer = match row.discord_id {
        Some(discord_id) => json!({
            "id": row.payer_id,
            "discord": { "id": discord_id.to_string() },
        }),
        None => json!({ "id": row.payer_id }),
    };

    let metadata = sqlx::query_scalar!(
        "SELECT metadata FROM claim_metadata WHERE claim_id = $1 AND owner_user_id = $2",
        claim_id,
        claimant_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .unwrap_or_else(|| json!({}));

    let event = json!({
        "id": row.id,
        "status": status,
        "amount": row.amount.unwrap_or_default().to_string(),
        "updated_at": timestamp(row.updated_at),
        "metadata": metadata,
        "payer": payer,
        "currency": {
            "id": row.currency_id,
            "unit": row.unit,
            "name": row.name,
            "guild": row.guild_id.map(|id| id.to_string()).unwrap_or_default(),
            "pool_amount": row.pool_amount.map(|amount| amount.to_string()).unwrap_or_default(),
        },
    });

    Ok(Some((row.claimant_id, event)))
}

/// How Jason writes a UTC `DateTime`: second precision with a trailing `Z`,
/// which is what `DateTime.from_naive!("Etc/UTC")` serializes to.
fn timestamp(value: PrimitiveDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        value.year(),
        value.month() as u8,
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
    )
}

/// `Money.approve_claim/3` and friends: lock, validate the operator, require
/// `pending`, move the money for an approval, then transition and set metadata.
///
/// The order matches the Elixir `with`: the operator is checked before the
/// status, and the transfer happens before the status update.
pub async fn transition(
    pool: &PgPool,
    notifier: &dyn Notifier,
    operator_id: i32,
    claim_id: i64,
    transition: Transition,
    metadata: Option<Value>,
) -> std::result::Result<(), TransitionError> {
    let mut tx = pool.begin().await.map_err(TransitionError::Database)?;

    let locked = match lock(&mut tx, claim_id)
        .await
        .map_err(TransitionError::Database)?
    {
        Some(locked) => locked,
        None => return Err(TransitionError::NotFound),
    };

    let operator = i64::from(operator_id);
    let allowed = if transition.requires_payer() {
        locked.payer_id == operator
    } else {
        locked.claimant_id == operator
    };
    if !allowed {
        return Err(TransitionError::InvalidOperator);
    }

    if locked.status.as_deref() != Some("pending") {
        return Err(TransitionError::InvalidStatus);
    }

    if matches!(transition, Transition::Approved) {
        crate::transfer::transfer(
            &mut tx,
            operator_id,
            i32::try_from(locked.claimant_id).map_err(|_| {
                TransitionError::Database(sqlx::Error::Protocol("claimant id out of range".into()))
            })?,
            locked.amount.unwrap_or_default(),
            locked.currency_unit.as_deref().unwrap_or_default(),
        )
        .await?;
    }

    update_status(
        &mut tx,
        operator_id,
        claim_id,
        transition.status(),
        metadata,
        locked.claimant_id,
        locked.payer_id,
    )
    .await?;

    // Build the event inside the payment transaction; a read failure must roll
    // back the payment too. Delivery happens only after a successful commit.
    let notification = if transition.requires_payer() {
        claim_notification(&mut tx, claim_id, locked.claimant_id)
            .await
            .map_err(TransitionError::Database)?
    } else {
        None
    };

    tx.commit().await.map_err(TransitionError::Database)?;

    if let Some((claimant_id, event)) = notification {
        notifier.notify_claim_update(claimant_id, &[event]);
    }

    Ok(())
}

/// `Money.update_metadata/3`: the metadata-only patch. Unlike a transition this
/// does not require the claim to be pending, and it never moves money.
pub async fn set_metadata(
    pool: &PgPool,
    operator_id: i32,
    claim_id: i64,
    metadata: Option<Value>,
) -> std::result::Result<(), TransitionError> {
    let mut tx = pool.begin().await.map_err(TransitionError::Database)?;

    let claim = sqlx::query!(
        "SELECT claimant_user_id AS \"claimant_id!\", payer_user_id AS \"payer_id!\"
           FROM claims WHERE id = $1",
        claim_id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(TransitionError::Database)?;

    let Some(claim) = claim else {
        return Err(TransitionError::NotFound);
    };

    let operator = i64::from(operator_id);
    if operator != claim.claimant_id && operator != claim.payer_id {
        return Err(TransitionError::InvalidOperator);
    }

    match metadata {
        Some(metadata) => {
            upsert_metadata(
                &mut tx,
                claim_id,
                claim.claimant_id,
                claim.payer_id,
                operator,
                metadata,
            )
            .await?;
        }
        None => delete_metadata(&mut tx, claim_id, operator).await?,
    }

    tx.commit().await.map_err(TransitionError::Database)?;

    Ok(())
}

/// One entry of a bulk status change: which claim, the status it should move to
/// (`None` changes nothing), and the operator's metadata when it should change.
#[derive(Debug, Clone, PartialEq)]
pub struct PartialClaim {
    pub id: i64,
    pub status: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug)]
pub enum UpdateClaimsError {
    /// The same claim id appears twice.
    DuplicatedClaims,
    /// A status outside `approved`, `denied`, `canceled` or `None`.
    InvalidStatus,
    InvalidMetadata(Vec<String>),
    NotFound,
    InvalidOperator,
    /// The operator is not the payer of every claim being approved or denied, or
    /// not the claimant of every claim being cancelled.
    PermissionDenied,
    /// One of the claims is no longer pending.
    InvalidCurrentStatus,
    NotFoundCurrency,
    NotFoundSenderAsset,
    NotEnoughAmount,
    InvalidAmount,
    Database(sqlx::Error),
}

impl From<TransferError> for UpdateClaimsError {
    fn from(error: TransferError) -> Self {
        match error {
            TransferError::InvalidAmount => UpdateClaimsError::InvalidAmount,
            TransferError::NotFoundCurrency => UpdateClaimsError::NotFoundCurrency,
            TransferError::NotFoundSenderAsset => UpdateClaimsError::NotFoundSenderAsset,
            TransferError::NotEnoughAmount => UpdateClaimsError::NotEnoughAmount,
            TransferError::Database(error) => UpdateClaimsError::Database(error),
        }
    }
}

impl From<TransitionError> for UpdateClaimsError {
    fn from(error: TransitionError) -> Self {
        match error {
            TransitionError::Database(error) => UpdateClaimsError::Database(error),
            // The `metadata_limitation` trigger rejecting the write, which the
            // controller reports as too many entries.
            TransitionError::MetadataLimit => UpdateClaimsError::InvalidMetadata(vec![
                "too many entries in metadata(max: 50)".to_string(),
            ]),
            other => UpdateClaimsError::Database(sqlx::Error::Protocol(format!(
                "unexpected error from the metadata write: {other:?}"
            ))),
        }
    }
}

/// What a bulk status change needs to know about each claim.
#[derive(Debug, Clone)]
struct BulkClaim {
    id: i64,
    status: Option<String>,
    amount: Option<i64>,
    currency_unit: Option<String>,
    claimant_id: i64,
    payer_id: i64,
}

/// `Money.update_claims/2`: apply several status changes at once, which is what
/// the claim-list buttons do.
///
/// The validations run in the order the Elixir `with` runs them — duplicate ids,
/// the statuses themselves, the metadata, that every claim exists, and that the
/// operator is a party to all of them — before anything is written. Approvals
/// then move every claim's money in one batch through
/// [`crate::transfer::transfer_bulk`], and one notification is dispatched per
/// claimant once the transaction has committed.
pub async fn update_claims(
    pool: &PgPool,
    notifier: &dyn Notifier,
    operator_id: i32,
    partial_claims: &[PartialClaim],
) -> std::result::Result<Vec<i64>, UpdateClaimsError> {
    let mut seen = std::collections::HashSet::new();
    for partial in partial_claims {
        if !seen.insert(partial.id) {
            return Err(UpdateClaimsError::DuplicatedClaims);
        }
    }

    for partial in partial_claims {
        match partial.status.as_deref() {
            None | Some("approved") | Some("denied") | Some("canceled") => {}
            Some(_) => return Err(UpdateClaimsError::InvalidStatus),
        }
    }

    let mut details = Vec::new();
    for (index, partial) in partial_claims.iter().enumerate() {
        if let Some(metadata) = &partial.metadata {
            details.extend(
                crate::metadata::validate(metadata)
                    .into_iter()
                    .map(|detail| format!("[{index}] {detail}")),
            );
        }
    }
    if !details.is_empty() {
        return Err(UpdateClaimsError::InvalidMetadata(details));
    }

    let time = utc_now();
    let mut tx = pool.begin().await.map_err(UpdateClaimsError::Database)?;

    let ids: Vec<i64> = partial_claims.iter().map(|partial| partial.id).collect();
    let rows = sqlx::query!(
        "SELECT c.id, c.status::text AS status, c.amount, cur.unit AS currency_unit,
                c.claimant_user_id AS \"claimant_id!\", c.payer_user_id AS \"payer_id!\"
           FROM claims c
           JOIN currencies cur ON c.currency_id = cur.id
          WHERE c.id = ANY($1)
            FOR UPDATE OF c",
        &ids
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(UpdateClaimsError::Database)?;

    if rows.len() != ids.len() {
        return Err(UpdateClaimsError::NotFound);
    }

    let claims: HashMap<i64, BulkClaim> = rows
        .into_iter()
        .map(|row| {
            (
                row.id,
                BulkClaim {
                    id: row.id,
                    status: row.status,
                    amount: row.amount,
                    currency_unit: row.currency_unit,
                    claimant_id: row.claimant_id,
                    payer_id: row.payer_id,
                },
            )
        })
        .collect();

    let operator = i64::from(operator_id);
    if !claims
        .values()
        .all(|claim| claim.claimant_id == operator || claim.payer_id == operator)
    {
        return Err(UpdateClaimsError::InvalidOperator);
    }

    let group = |status: &str| -> Vec<&PartialClaim> {
        partial_claims
            .iter()
            .filter(|partial| partial.status.as_deref() == Some(status))
            .collect()
    };

    let mut updated: Vec<i64> = Vec::new();

    let approving = group("approved");
    if !approving.is_empty() {
        let approving: Vec<&BulkClaim> = approving.iter().map(|p| &claims[&p.id]).collect();

        if !approving.iter().all(|claim| claim.payer_id == operator) {
            return Err(UpdateClaimsError::PermissionDenied);
        }
        if !approving
            .iter()
            .all(|claim| claim.status.as_deref() == Some("pending"))
        {
            return Err(UpdateClaimsError::InvalidCurrentStatus);
        }

        // `approve_claims/3` sends from the first claim's payer, which every
        // entry has just been checked against.
        let sender_id = i32::try_from(approving[0].payer_id).map_err(|_| {
            UpdateClaimsError::Database(sqlx::Error::Protocol("payer id out of range".into()))
        })?;

        let entries: Vec<crate::transfer::BulkEntry> = approving
            .iter()
            .map(|claim| {
                Ok((
                    claim.currency_unit.clone().unwrap_or_default(),
                    i32::try_from(claim.claimant_id).map_err(|_| {
                        UpdateClaimsError::Database(sqlx::Error::Protocol(
                            "claimant id out of range".into(),
                        ))
                    })?,
                    claim.amount.unwrap_or_default(),
                ))
            })
            .collect::<std::result::Result<Vec<_>, UpdateClaimsError>>()?;

        crate::transfer::transfer_bulk(&mut tx, sender_id, &entries)
            .await
            .map_err(UpdateClaimsError::from)?;

        let approving_ids: Vec<i64> = approving.iter().map(|claim| claim.id).collect();
        update_statuses(&mut tx, &approving_ids, "approved", time).await?;
        updated.extend(approving_ids);
    }

    let denying = group("denied");
    if !denying.is_empty() {
        let denying: Vec<&BulkClaim> = denying.iter().map(|p| &claims[&p.id]).collect();

        if !denying.iter().all(|claim| claim.payer_id == operator) {
            return Err(UpdateClaimsError::PermissionDenied);
        }
        if !denying
            .iter()
            .all(|claim| claim.status.as_deref() == Some("pending"))
        {
            return Err(UpdateClaimsError::InvalidCurrentStatus);
        }

        let denying_ids: Vec<i64> = denying.iter().map(|claim| claim.id).collect();
        update_statuses(&mut tx, &denying_ids, "denied", time).await?;
        updated.extend(denying_ids);
    }

    let canceling = group("canceled");
    if !canceling.is_empty() {
        let canceling: Vec<&BulkClaim> = canceling.iter().map(|p| &claims[&p.id]).collect();

        if !canceling.iter().all(|claim| claim.claimant_id == operator) {
            return Err(UpdateClaimsError::PermissionDenied);
        }
        if !canceling
            .iter()
            .all(|claim| claim.status.as_deref() == Some("pending"))
        {
            return Err(UpdateClaimsError::InvalidCurrentStatus);
        }

        let canceling_ids: Vec<i64> = canceling.iter().map(|claim| claim.id).collect();
        update_statuses(&mut tx, &canceling_ids, "canceled", time).await?;
        updated.extend(canceling_ids);
    }

    // The operator's own metadata, for the claims that asked it to change.
    for partial in partial_claims
        .iter()
        .filter(|partial| partial.metadata.is_some())
    {
        let claim = &claims[&partial.id];

        upsert_metadata(
            &mut tx,
            claim.id,
            claim.claimant_id,
            claim.payer_id,
            operator,
            partial.metadata.clone().unwrap_or_default(),
        )
        .await
        .map_err(UpdateClaimsError::from)?;
    }

    // Prepare events before committing, so notification reads cannot fail after
    // the payments have already been persisted.
    // One notification per claimant, carrying every event for that claimant. A
    // cancelled claim has no event, which is how it drops out here.
    let mut grouped: Vec<(i32, Vec<Value>)> = Vec::new();
    for id in &updated {
        let claim = &claims[id];

        let Some((claimant_id, event)) = claim_notification(&mut tx, *id, claim.claimant_id)
            .await
            .map_err(UpdateClaimsError::Database)?
        else {
            continue;
        };

        match grouped.iter_mut().find(|(group, _)| *group == claimant_id) {
            Some((_, events)) => events.push(event),
            None => grouped.push((claimant_id, vec![event])),
        }
    }

    tx.commit().await.map_err(UpdateClaimsError::Database)?;

    for (claimant_id, events) in grouped {
        notifier.notify_claim_update(claimant_id, &events);
    }

    Ok(updated)
}

/// `Query.Claim.update_claims_status/3`: one statement for the whole group.
async fn update_statuses(
    conn: &mut PgConnection,
    ids: &[i64],
    status: &str,
    time: PrimitiveDateTime,
) -> std::result::Result<(), UpdateClaimsError> {
    sqlx::query!(
        "UPDATE claims
            SET status = $2::text::virtual_crypto_claim_status, updated_at = $3
          WHERE id = ANY($1)",
        ids,
        status,
        time
    )
    .execute(&mut *conn)
    .await
    .map_err(UpdateClaimsError::Database)?;

    Ok(())
}

/// `Query.Claim.list_candidates/6`: the claims whose id starts with `query`,
/// the ones in the caller's own guild first and the newest first after that.
///
/// `filter` is which side of the claim the caller has to be, so the same `id`
/// option offers what the caller could act on from where they are standing.
pub async fn search_candidates(
    pool: &PgPool,
    operator_discord_id: i64,
    query: &str,
    filter: SrFilter,
    statuses: &[String],
    guild_id: Option<i64>,
    limit: i64,
) -> Result<Vec<ClaimView>> {
    let mut prefix = String::with_capacity(query.len() + 1);
    for character in query.chars() {
        if matches!(character, '\\' | '%' | '_') {
            prefix.push('\\');
        }
        prefix.push(character);
    }
    prefix.push('%');

    let rows = sqlx::query_as!(
        Row,
        "SELECT c.id AS \"claim_id!\",
                c.amount AS claim_amount,
                c.status::text AS claim_status,
                c.inserted_at AS \"claim_inserted_at!\",
                c.updated_at AS \"claim_updated_at!\",
                cur.name AS currency_name,
                cur.unit AS currency_unit,
                cur.guild_id AS currency_guild_id,
                cur.pool_amount AS currency_pool_amount,
                cl.id AS \"claimant_id!\",
                cl.discord_id AS claimant_discord_id,
                py.id AS \"payer_id!\",
                py.discord_id AS payer_discord_id,
                '{}'::jsonb AS \"metadata!\"
           FROM claims c
           JOIN currencies cur ON c.currency_id = cur.id
           JOIN users cl ON c.claimant_user_id = cl.id
           JOIN users py ON c.payer_user_id = py.id
          WHERE c.status::text = ANY($1)
            AND ($2 = 'all'
                 AND (cl.discord_id = $3 OR py.discord_id = $3)
              OR $2 = 'received' AND py.discord_id = $3
              OR $2 = 'claimed' AND cl.discord_id = $3)
            AND c.id::text LIKE $4
          ORDER BY (cur.guild_id = $5) DESC NULLS LAST, c.id DESC
          LIMIT $6",
        statuses,
        filter.as_str(),
        operator_discord_id,
        prefix,
        guild_id,
        limit
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Row::into_view).collect())
}
