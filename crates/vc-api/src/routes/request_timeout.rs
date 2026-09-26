//! A normal request timeout bounds how long v2 may use an authorization result.
//! It covers extractors and the body as well as the handler. Dropping a handler
//! drops its open SQLx transaction; the runtime pool's statement timeout also
//! prevents its outstanding query from holding a connection indefinitely.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;

pub(super) async fn enforce(request: Request, next: Next) -> Response {
    match tokio::time::timeout(vc_core::db::QUERY_TIMEOUT, next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::RequestTimeout.into_response(),
    }
}
