use serde_json::Value;
use sqlx::PgPool;
use sqlx::types::Json;
use time::PrimitiveDateTime;

use crate::error::Result;

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
