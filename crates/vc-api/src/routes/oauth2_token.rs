//! `POST /oauth2/token`: the grants a client can use.
//!
//! Three of the four grants are here: the code exchange, the refresh, and
//! `client_credentials` in both of its shapes — which are two mechanisms wearing
//! one name, and are documented as such in docs/oauth2.md.
//!
//! Revocation is not here yet.

use axum::Json;
use axum::extract::{Form, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use time::OffsetDateTime;

use vc_auth::issue::{app_scopes_are_valid, app_token, revoke_by_jti};
use vc_core::application::{Application, verify_secret};
use vc_core::grant::{
    EXPIRES_IN, ExchangeError, create_access_token, exchange_code, exchange_refresh_token,
    revoke_access_token, revoke_refresh_token,
};

use crate::state::AppState;

/// The body an authorization request arrives as.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TokenForm {
    pub grant_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code: Option<String>,
    pub refresh_token: Option<String>,
    pub device_code: Option<String>,
    #[allow(dead_code)]
    pub scope: Option<String>,
}

/// `POST /oauth2/token`.
pub async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    match form.grant_type.as_deref() {
        None => error("invalid_request", "grant_type_parameter_missing"),
        Some("authorization_code") => exchange(&state, form).await,
        Some("refresh_token") => refresh(&state, form).await,
        Some("client_credentials") => credentials(&state, &headers, form).await,
        Some("urn:ietf:params:oauth:grant-type:device_code") => {
            device(&state, &headers, form).await
        }
        Some(_) => unsupported(),
    }
}

/// `grant_type=client_credentials`, which answers with a signed JWT for the
/// application itself — the scope shape, and the only one left. The guild shape
/// is gone: it minted a guild token for a grant nobody asked to exist, and the
/// device poll below is what mints one now, only once the guild approved the
/// ask. See `docs/oauth2.md`.
///
/// The credentials are read first because every shape refuses a client that
/// presents none the same way, and that much is worth answering correctly now:
/// `invalid_client`, which is what the Elixir answers for a header it cannot
/// parse.
async fn credentials(state: &AppState, headers: &HeaderMap, form: TokenForm) -> Response {
    let Some((client_id, client_secret)) = basic_auth(headers) else {
        return invalid_client();
    };

    let verified = verify_secret(state.pool(), &client_id, &client_secret)
        .await
        .ok()
        .flatten();

    let Some(application) = verified else {
        return invalid_client();
    };

    if let Some(scope) = form.scope {
        return scoped(state, &application, &scope).await;
    }

    unsupported()
}

/// `grant_type=urn:ietf:params:oauth:grant-type:device_code`: the device poll.
///
/// The application names the `device_code` its ask answered with, authenticated
/// the way every direct call to this endpoint is — Basic with its own id and
/// secret. What comes back depends on what the guild has done with the ask:
///
/// - still pending: `400 authorization_pending`, and the device keeps polling;
/// - approved: the guild token, minted from the grant the approval wrote;
/// - unknown, expired, or approved-but-revoked: `400 invalid_grant`, and the
///   device must start over with a new ask.
///
/// The failures after the first are one answer on purpose: a `device_code` that
/// never existed and one whose ask died are indistinguishable to anyone but the
/// application that made it, and the application knows which of its own asks is
/// which.
async fn device(state: &AppState, headers: &HeaderMap, form: TokenForm) -> Response {
    let Some((client_id, client_secret)) = basic_auth(headers) else {
        return invalid_client();
    };

    let verified = verify_secret(state.pool(), &client_id, &client_secret)
        .await
        .ok()
        .flatten();

    let Some(application) = verified else {
        return invalid_client();
    };

    let Some(device_code) = form.device_code.and_then(|code| code.parse().ok()) else {
        return error("invalid_request", "device_code");
    };

    let polled = vc_core::grant::poll_request(
        state.pool(),
        application.id,
        device_code,
        OffsetDateTime::now_utc(),
    )
    .await
    .ok()
    .flatten();

    let Some(asked) = polled else {
        return error("invalid_grant", "invalid_device_code");
    };

    if asked.status == "pending" {
        return error("authorization_pending", "authorization_pending");
    }

    if asked.status != "approved" {
        return error("invalid_grant", "invalid_device_code");
    }

    let Some(grant_id) = vc_core::grant::grant_for(state.pool(), application.id, asked.guild_id)
        .await
        .ok()
        .flatten()
    else {
        // The ask is approved but the grant is gone: it was revoked between the
        // two reads. The device must ask again rather than wait on an approval
        // that no longer means anything.
        return error("invalid_grant", "invalid_device_code");
    };

    let now = OffsetDateTime::now_utc();

    let Ok(access_token) = create_access_token(state.pool(), grant_id, now).await else {
        return invalid_client();
    };

    Json(json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": EXPIRES_IN,
    }))
    .into_response()
}

/// The shape that answers with a signed JWT, for the application itself rather
/// than for a guild.
async fn scoped(state: &AppState, application: &Application, scope: &str) -> Response {
    // `String.split/2` on the empty string gives one empty scope, so a request
    // with `scope=` is refused rather than treated as asking for nothing.
    let scopes: Vec<&str> = scope.split(' ').collect();

    if !app_scopes_are_valid(&scopes) {
        // No description: the credentials view renders this error alone.
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_request" })),
        )
            .into_response();
    }

    let subject = vc_core::user::application_user_id(state.pool(), application.id)
        .await
        .ok()
        .flatten();

    let Some(subject) = subject else {
        return invalid_client();
    };

    let issued = app_token(
        state.pool(),
        state.jwt_secret(),
        i64::from(subject),
        &scopes,
        OffsetDateTime::now_utc(),
    )
    .await;

    match issued {
        Ok(access_token) => Json(json!({
            "access_token": access_token,
            "expires_in": EXPIRES_IN,
            "token_type": "Bearer",
        }))
        .into_response(),
        Err(_) => invalid_client(),
    }
}

fn invalid_client() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "invalid_client" })),
    )
        .into_response()
}

/// `grant_type=authorization_code`.
async fn exchange(state: &AppState, form: TokenForm) -> Response {
    // Which parameter is missing is what the client is told, in the Elixir's
    // order.
    let Some(client_id) = form.client_id else {
        return error("invalid_request", "client_id");
    };
    let Some(redirect_uri) = form.redirect_uri else {
        return error("invalid_request", "redirect_uri");
    };
    let Some(code) = form.code else {
        return error("invalid_request", "code");
    };

    let exchanged = exchange_code(
        state.pool(),
        &client_id,
        &redirect_uri,
        &code,
        OffsetDateTime::now_utc(),
    )
    .await;

    match exchanged {
        Ok(exchanged) => {
            let mut body = json!({
                "access_token": exchanged.access_token,
                "token_type": "Bearer",
                "expires_in": exchanged.expires_in,
                "scopes": exchanged.scopes,
            });

            if let Some(refresh_token) = exchanged.refresh_token {
                body["refresh_token"] = json!(refresh_token);
            }

            Json(body).into_response()
        }
        Err(error) => refused(error),
    }
}

/// `grant_type=refresh_token`.
async fn refresh(state: &AppState, form: TokenForm) -> Response {
    let Some(refresh_token) = form.refresh_token else {
        return error("invalid_request", "refresh_token");
    };

    let refreshed =
        exchange_refresh_token(state.pool(), &refresh_token, OffsetDateTime::now_utc()).await;

    match refreshed {
        Ok(refreshed) => Json(json!({
            "access_token": refreshed.access_token,
            "token_type": "Bearer",
            // The literal the Elixir writes here, rather than the clock the
            // credentials path uses. Reproduced, not harmonised.
            "expires_in": EXPIRES_IN,
            "refresh_token": refreshed.refresh_token,
        }))
        .into_response(),
        Err(error) => refused(error),
    }
}

/// A grant this endpoint does not have. The Elixir's answer carries no
/// description for this one.
fn unsupported() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "unsupported_grant_type" })),
    )
        .into_response()
}

fn error(error: &str, description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

/// A refusal from the exchange.
///
/// Answered `400`, where the Elixir renders the same body with whatever status
/// the connection already had — which is `200`. RFC 6749 has `invalid_grant` at
/// `400`, and a client library that checks the status is the reason to follow it:
/// an error body under a success status is read as a success by exactly the
/// clients this endpoint exists for. The other grants in the Elixir do set `400`,
/// so this is the odd one out rather than the rule. Recorded in
/// docs/known-gaps.md.
fn refused(error: ExchangeError) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": error.error(),
            "error_description": error.description(),
        })),
    )
        .into_response()
}

/// `Plug.BasicAuth.parse_basic_auth/1`: the client id and secret a client
/// presents.
///
/// The scheme is matched case-insensitively because RFC 7235 says it is, and a
/// client that writes `basic` is a client that is right. The split is on the
/// *first* colon: a client id cannot contain one, and a secret can.
fn basic_auth(headers: &HeaderMap) -> Option<(String, String)> {
    let header = headers.get(AUTHORIZATION)?.to_str().ok()?;

    let (scheme, encoded) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }

    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;

    let (client_id, client_secret) = decoded.split_once(':')?;

    Some((client_id.to_owned(), client_secret.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            axum::http::header::HeaderValue::from_str(value).expect("a header"),
        );
        headers
    }

    #[test]
    fn an_id_and_a_secret_are_read_out() {
        // "client:secret"
        let parsed = basic_auth(&headers("Basic Y2xpZW50OnNlY3JldA=="));

        assert_eq!(parsed, Some(("client".to_owned(), "secret".to_owned())));
    }

    /// A secret may contain a colon and the id may not, which is why the split
    /// is on the first one.
    #[test]
    fn a_secret_may_contain_a_colon() {
        // "client:sec:ret"
        let parsed = basic_auth(&headers("Basic Y2xpZW50OnNlYzpyZXQ="));

        assert_eq!(parsed, Some(("client".to_owned(), "sec:ret".to_owned())));
    }

    /// RFC 7235 makes the scheme case-insensitive.
    #[test]
    fn the_scheme_is_matched_case_insensitively() {
        assert!(basic_auth(&headers("basic Y2xpZW50OnNlY3JldA==")).is_some());
        assert!(basic_auth(&headers("BASIC Y2xpZW50OnNlY3JldA==")).is_some());
    }

    #[test]
    fn anything_else_is_not_credentials() {
        assert_eq!(basic_auth(&HeaderMap::new()), None);
        assert_eq!(basic_auth(&headers("Bearer a-token")), None);
        assert_eq!(basic_auth(&headers("Basic not-base64!!")), None);
        assert_eq!(basic_auth(&headers("Basic Y2xpZW50")), None, "no colon");
    }
}

/// The body `POST /oauth2/token/revoke` arrives as.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RevokeForm {
    pub token: Option<String>,
    pub jti: Option<String>,
    pub typ: Option<String>,
    pub kind: Option<String>,
}

/// `POST /oauth2/token/revoke`.
///
/// Two shapes, as the Elixir has. The one that takes a plain `token` goes
/// further here: the Elixir sends it to `Guardian.revoke/1`, which can only
/// verify a JWT, so the tokens `POST /oauth2/token` issues were not revocable by
/// the endpoint that exists to revoke them. This tries the JWT first and then
/// the two rows this service issues.
///
/// A `token` that is none of those is still `200`: RFC 7009 says a revocation
/// endpoint must not say whether the token existed, and a client that has
/// already forgotten a token should not be told off for tidying up.
pub async fn revoke(State(state): State<AppState>, Form(form): Form<RevokeForm>) -> Response {
    if let Some(token) = form.token {
        if let Ok(claims) = vc_auth::jwt::verify(&token, state.jwt_secret()) {
            let _ = revoke_by_jti(state.pool(), &claims.jti).await;

            return revoked();
        }

        let _ = revoke_access_token(state.pool(), &token).await;
        let _ = revoke_refresh_token(state.pool(), &token).await;

        return revoked();
    }

    if let (Some(jti), Some(typ), Some(kind)) = (form.jti, form.typ, form.kind)
        && typ == "access"
        && matches!(kind.as_str(), "app" | "user")
    {
        let _ = revoke_by_jti(state.pool(), &jti).await;

        return revoked();
    }

    // The Elixir's own sentence, which is one word longer than it needs to be and
    // is what a client may already be matching on.
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "invalid_request",
            "error_description":
                "token_or_token_id_type_and_kind_is_not_found_or_invalid_kind_or_type",
        })),
    )
        .into_response()
}

/// The answer to a revocation that happened, or to one that had nothing to do.
fn revoked() -> Response {
    Json(json!({})).into_response()
}
