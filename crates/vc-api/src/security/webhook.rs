//! Durable FIFO delivery of the operator's security notifications.
//! Enqueueing commits before returning. A transaction-scoped advisory lock
//! serializes senders across processes, including the HTTP request. A crash
//! after remote acceptance and before commit can therefore repeat a notification.

use std::time::{Duration, SystemTime};

use reqwest::{Client, Response, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::Notify;

use super::{Report, payload};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_BODY: usize = 4096;
const DELIVERY_LOCK: i64 = 0x7663_7365_6375_7265;

struct Destination {
    pool: PgPool,
    url: String,
    http: Client,
    ready: Notify,
}

/// The URL stays in deployment configuration, never in the queue or logs.
/// Pending reports use the configured destination when a worker resumes.
#[derive(Default)]
pub struct WebhookSink {
    destination: Option<Destination>,
}

impl WebhookSink {
    pub(super) fn new(pool: PgPool, url: Option<String>) -> Self {
        Self {
            destination: url
                .filter(|url| !url.trim().is_empty())
                .map(|url| Destination {
                    pool,
                    url,
                    http: Client::new(),
                    ready: Notify::new(),
                }),
        }
    }

    pub(super) async fn enqueue(&self, reports: &[Report]) -> Result<(), sqlx::Error> {
        let Some(destination) = &self.destination else {
            return Ok(());
        };
        if reports.is_empty() {
            return Ok(());
        }
        let batch = Value::Array(
            reports
                .iter()
                .map(|report| {
                    json!({
                        "signal": report.signal.name(), "body": payload(&destination.url, report)
                    })
                })
                .collect(),
        );
        sqlx::query!(
            "INSERT INTO security_webhook_queue (signal, body)
             SELECT report->>'signal', report->'body'
               FROM jsonb_array_elements($1) WITH ORDINALITY AS batch(report, position)
              ORDER BY position",
            batch
        )
        .execute(&destination.pool)
        .await?;
        destination.ready.notify_one();
        Ok(())
    }

    pub(super) async fn run(&self) {
        let Some(destination) = &self.destination else {
            return;
        };
        loop {
            match destination.process_one().await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => tracing::warn!(target: "vc_security", %error,
                    "the security webhook queue could not be processed"),
            }
            // Polling also picks up rows inserted by another machine, or before
            // this process started. Local enqueueing wakes the worker sooner.
            tokio::select! {
                _ = destination.ready.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(1)) => {},
            }
        }
    }
}

impl Destination {
    /// True when one report was handled. The oldest pending report holds the
    /// queue during a retry delay, so later reports cannot bypass a 429.
    async fn process_one(&self) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let claimed = sqlx::query_scalar!(
            "SELECT pg_try_advisory_xact_lock($1) AS \"claimed!\"",
            DELIVERY_LOCK
        )
        .fetch_one(&mut *tx)
        .await?;
        if !claimed {
            tx.rollback().await?;
            return Ok(false);
        }
        let row = sqlx::query!(
            "SELECT id, signal, body, attempts, next_attempt_at <= now() AS \"due!\"
               FROM security_webhook_queue WHERE failed_at IS NULL
              ORDER BY id LIMIT 1 FOR UPDATE"
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(false);
        };
        if !row.due {
            tx.rollback().await?;
            return Ok(false);
        }

        let attempt = row.attempts.saturating_add(1);
        let response = self
            .http
            .post(&self.url)
            .timeout(REQUEST_TIMEOUT)
            .json(&row.body)
            .send()
            .await;
        let (retry, status) = match response {
            Ok(response) if response.status().is_success() => {
                sqlx::query!("DELETE FROM security_webhook_queue WHERE id = $1", row.id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                return Ok(true);
            }
            Ok(response) => {
                let status = response.status();
                tracing::warn!(target: "vc_security", signal = row.signal, notification_id = row.id,
                    attempt, status = status.as_u16(), "the security webhook refused a notification");
                let retry = if status == StatusCode::TOO_MANY_REQUESTS
                    || status == StatusCode::REQUEST_TIMEOUT
                    || status.is_server_error()
                {
                    Some(response_retry_after(response).await.unwrap_or_default())
                } else {
                    None
                };
                (retry, Some(status.as_u16() as i16))
            }
            Err(error) => {
                let invalid_configuration = error.is_builder();
                let error = error.without_url();
                tracing::warn!(target: "vc_security", signal = row.signal, notification_id = row.id,
                    attempt, "the security webhook could not be reached: {error}");
                ((!invalid_configuration).then_some(Duration::ZERO), None)
            }
        };

        if let Some(delay) = retry {
            let backoff = Duration::from_secs(2_u64.saturating_pow((attempt - 1) as u32).min(300));
            let delay = delay.max(backoff).as_secs_f64();
            sqlx::query!(
                "UPDATE security_webhook_queue SET attempts = $2, last_status = $3,
                        next_attempt_at = clock_timestamp() + make_interval(secs => $4)
                  WHERE id = $1",
                row.id,
                attempt,
                status,
                delay
            )
            .execute(&mut *tx)
            .await?;
            tracing::warn!(target: "vc_security", signal = row.signal, notification_id = row.id,
                attempt, retry_after_secs = delay, "the security webhook notification will be retried");
        } else {
            sqlx::query!(
                "UPDATE security_webhook_queue SET attempts = $2, last_status = $3, failed_at = now()
                  WHERE id = $1", row.id, attempt, status
            ).execute(&mut *tx).await?;
            tracing::warn!(target: "vc_security", signal = row.signal, notification_id = row.id,
                "the security webhook notification failed permanently");
        }
        tx.commit().await?;
        Ok(true)
    }
}

fn seconds(value: f64) -> Option<Duration> {
    let delay = Duration::try_from_secs_f64(value).ok()?;
    // Reject intervals outside the timestamp range before persisting them.
    time::OffsetDateTime::now_utc().checked_add(time::Duration::try_from(delay).ok()?)?;
    Some(delay)
}

fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(value) = value.parse::<f64>() {
        return seconds(value);
    }
    let at = httpdate::parse_http_date(value).ok()?;
    let delay = at.duration_since(now).unwrap_or_default();
    time::OffsetDateTime::now_utc().checked_add(time::Duration::try_from(delay).ok()?)?;
    Some(delay)
}

async fn response_retry_after(mut response: Response) -> Option<Duration> {
    if let Some(delay) = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| retry_after(value, SystemTime::now()))
    {
        return Some(delay);
    }
    if response.status() != StatusCode::TOO_MANY_REQUESTS {
        return None;
    }

    // Discord also supplies fractional seconds in a JSON retry_after field:
    // https://docs.discord.com/developers/topics/rate-limits
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if chunk.len() > MAX_ERROR_BODY - body.len() {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&body).ok()?;
    seconds(body.get("retry_after")?.as_f64()?)
}

#[cfg(test)]
mod tests;
