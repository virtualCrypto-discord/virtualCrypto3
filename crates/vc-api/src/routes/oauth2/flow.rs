//! One browser-bound authorization, never a reusable login credential.
use axum::http::{HeaderMap, HeaderValue, header};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::Request;

#[derive(Serialize, Deserialize)]
pub(super) struct Pending {
    pub request: Request,
    pub resource_uris: Vec<String>,
}

fn cookie_name(id: Uuid) -> String {
    format!("_vc_authorize_{}", id.simple())
}

pub(super) fn binding(headers: &HeaderMap, id: Uuid) -> Option<Uuid> {
    let name = cookie_name(id);
    let mut values = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(key, _)| *key == name);
    let (_, value) = values.next()?;
    if values.next().is_some() {
        return None;
    }
    Uuid::parse_str(value).ok()
}

pub(super) fn cookie(id: Uuid, secret: Uuid, secure: bool) -> HeaderValue {
    format!(
        "{}={secret}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{}",
        cookie_name(id),
        if secure { "; Secure" } else { "" }
    )
    .parse()
    .expect("UUID cookie")
}

pub(super) fn clear_cookie(id: Uuid, secure: bool) -> HeaderValue {
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        cookie_name(id),
        if secure { "; Secure" } else { "" }
    )
    .parse()
    .expect("UUID cookie")
}

pub(super) async fn start(pool: &PgPool, pending: Pending) -> Result<(Uuid, Uuid), sqlx::Error> {
    let id = Uuid::new_v4();
    let secret = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO browser_authorizations (id, browser_secret, request) VALUES ($1, $2, $3)",
    )
    .bind(id)
    .bind(secret)
    .bind(sqlx::types::Json(pending))
    .execute(pool)
    .await?;
    Ok((id, secret))
}

/// Claim before contacting Discord. Concurrent callbacks cannot exchange twice.
/// A crash or failed exchange requires a fresh authorization request.
pub(super) async fn claim(
    pool: &PgPool,
    id: Uuid,
    secret: Uuid,
) -> Result<Option<Pending>, sqlx::Error> {
    let pending = sqlx::query_scalar::<_, sqlx::types::Json<Pending>>(
        "UPDATE browser_authorizations SET phase = 'verifying' WHERE id=$1 AND browser_secret=$2
         AND phase='discord' AND expires > clock_timestamp() RETURNING request",
    )
    .bind(id)
    .bind(secret)
    .fetch_optional(pool)
    .await?;
    Ok(pending.map(|pending| pending.0))
}

pub(super) async fn ready(pool: &PgPool, id: Uuid, account: i64) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE browser_authorizations SET phase='consent', account_id=$2
        WHERE id=$1 AND phase='verifying' AND expires > clock_timestamp()",
    )
    .bind(id)
    .bind(account)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

pub(super) async fn load(
    pool: &PgPool,
    id: Uuid,
    secret: Uuid,
) -> Result<Option<(Pending, i64)>, sqlx::Error> {
    let row = sqlx::query_as::<_, (sqlx::types::Json<Pending>, i64)>(
        "SELECT request, account_id FROM browser_authorizations WHERE id=$1 AND browser_secret=$2
         AND phase='consent' AND expires > clock_timestamp()",
    )
    .bind(id)
    .bind(secret)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(pending, account)| (pending.0, account)))
}

/// Consume inside the code's transaction. Only one approval may commit.
pub(super) async fn consume(
    conn: &mut PgConnection,
    id: Uuid,
    secret: Uuid,
) -> Result<bool, sqlx::Error> {
    // Check the deadline after acquiring the lock, not before a possible wait.
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM browser_authorizations WHERE id=$1 AND browser_secret=$2
         AND phase='consent' FOR UPDATE",
    )
    .bind(id)
    .bind(secret)
    .fetch_optional(&mut *conn)
    .await?;
    if locked.is_none() {
        return Ok(false);
    }
    Ok(sqlx::query(
        "DELETE FROM browser_authorizations WHERE id=$1 AND browser_secret=$2
        AND phase='consent' AND expires > clock_timestamp()",
    )
    .bind(id)
    .bind(secret)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}
