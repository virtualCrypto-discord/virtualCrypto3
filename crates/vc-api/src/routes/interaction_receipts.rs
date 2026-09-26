//! At-most-once dispatch across workers and restarts. A receipt is committed
//! before the handler starts; completed HTTP responses can be replayed.
//!
//! A crash between dispatch and recording the response leaves a pending receipt.
//! It must not be reclaimed automatically: the command may already have committed
//! its payment. This does not promise exactly-once completion. Receipts and their
//! response bodies are purged after 24 hours; the endpoint refuses signed
//! requests older than five minutes, including after their receipt is gone.
//! Neither the interaction token nor its request body is stored here.
//! For commands acknowledged through Discord's callback, the saved 202 records
//! acceptance, not completion; replay must not restart their background work.

use axum::body::{Body, to_bytes};
use axum::http::{StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::state::AppState;

pub(super) async fn run(state: AppState, payload: Value) -> Response {
    let Some(id) = payload.get("id").and_then(Value::as_str).filter(|id| {
        !id.is_empty() && id.len() <= 20 && id.bytes().all(|byte| byte.is_ascii_digit())
    }) else {
        return (StatusCode::BAD_REQUEST, "Missing or invalid interaction ID").into_response();
    };
    let id = id.to_owned();

    // Disconnecting the HTTP request must not cancel the handler after its
    // receipt or one of its writes has committed.
    match tokio::spawn(async move { dispatch(&state, &payload, &id).await }).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            tracing::error!(%error, "interaction receipt failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(error) => {
            tracing::error!(%error, "interaction task failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn dispatch(state: &AppState, payload: &Value, id: &str) -> Result<Response, sqlx::Error> {
    let claimed = sqlx::query!(
        "INSERT INTO discord_interactions (id) VALUES ($1) ON CONFLICT DO NOTHING",
        id
    )
    .execute(state.pool())
    .await?
    .rows_affected();

    if claimed == 0 {
        // Discord retries on its own, and a receipt still pending answers 409 —
        // so one duplicate is ordinary traffic. What is warned about is the
        // repetition, which the signal's own threshold decides: replay is
        // somebody resending what was already accepted, not a client's manners.
        state
            .monitor()
            .observe(
                crate::security::Signal::ReplayDuplicate,
                "interaction-receipts",
                "an interaction id was already received",
            )
            .await;

        let receipt = sqlx::query!(
            "SELECT status, content_type, body FROM discord_interactions WHERE id = $1",
            id
        )
        .fetch_one(state.pool())
        .await?;
        let (Some(status), Some(body)) = (receipt.status, receipt.body) else {
            return Ok((StatusCode::CONFLICT, "Interaction already accepted").into_response());
        };
        let mut response = Response::new(Body::from(body));
        *response.status_mut() = StatusCode::from_u16(status as u16).expect("checked status");
        if let Some(content_type) = receipt.content_type {
            response.headers_mut().insert(
                CONTENT_TYPE,
                content_type.parse().expect("stored response content type"),
            );
        }
        return Ok(response);
    }

    let response = super::interactions::dispatch(state, payload).await;
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX)
        .await
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let status = i32::from(parts.status.as_u16());
    let content_type = parts
        .headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    sqlx::query!(
        "UPDATE discord_interactions SET status = $2, content_type = $3, body = $4 WHERE id = $1",
        id,
        status,
        content_type,
        body.as_ref()
    )
    .execute(state.pool())
    .await?;
    Ok(Response::from_parts(parts, Body::from(body)))
}
