use serde_json::Value;
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool};
use time::PrimitiveDateTime;

use crate::error::Result;
use crate::model::utc_now;
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

/// Where to resume from. `First` is the start, `Next` is exclusive (`<`/`>`) and
/// `OnNext` is inclusive (`<=`/`>=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    First,
    Next(i64),
    OnNext(i64),
}

impl Cursor {
    fn kind(self) -> &'static str {
        match self {
            Cursor::First => "first",
            Cursor::Next(_) => "next",
            Cursor::OnNext(_) => "on_next",
        }
    }

    fn value(self) -> Option<i64> {
        match self {
            Cursor::First => None,
            Cursor::Next(value) | Cursor::OnNext(value) => Some(value),
        }
    }
}

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

/// `Money.approve_claim/3` and friends: lock, validate the operator, require
/// `pending`, move the money for an approval, then transition and set metadata.
///
/// The order matches the Elixir `with`: the operator is checked before the
/// status, and the transfer happens before the status update.
pub async fn transition(
    pool: &PgPool,
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

    tx.commit().await.map_err(TransitionError::Database)?;

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
