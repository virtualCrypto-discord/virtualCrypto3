//! RFC 8414 discovery for this OAuth authorization server (not OpenID Connect).
use axum::{Json, extract::State, http::header::ACCESS_CONTROL_ALLOW_ORIGIN};
use serde_json::json;

use crate::state::AppState;

/// Use trusted deployment configuration, never the incoming Host/Forwarded headers.
pub(super) fn issuer(state: &AppState) -> &str {
    state.links().site_url.trim_end_matches('/')
}

pub async fn metadata(State(state): State<AppState>) -> impl axum::response::IntoResponse {
    let issuer = issuer(&state);
    (
        [(ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        Json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/oauth2/authorize"),
            "token_endpoint": format!("{issuer}/oauth2/token"),
            "revocation_endpoint": format!("{issuer}/oauth2/token/revoke"),
            "revocation_endpoint_auth_methods_supported": ["none"],
            "registration_endpoint": format!("{issuer}/oauth2/clients"),
            "response_types_supported": ["code"],
            "response_modes_supported": ["query"],
            "grant_types_supported": ["authorization_code", "refresh_token", "client_credentials", "urn:ietf:params:oauth:grant-type:device_code"],
            "token_endpoint_auth_methods_supported": ["none", "client_secret_basic"],
            "code_challenge_methods_supported": ["S256"],
            "authorization_response_iss_parameter_supported": true
        })),
    )
}
