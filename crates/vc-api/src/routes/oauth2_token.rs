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

use vc_auth::issue::{app_scopes_are_valid, app_token};
use vc_core::application::{Application, verify_secret};
use vc_core::grant::{
    EXPIRES_IN, ExchangeError, create_access_token, create_refresh_token, exchange_code,
    exchange_refresh_token, grant_for,
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
    #[allow(dead_code)]
    pub scope: Option<String>,
    #[allow(dead_code)]
    pub guild_id: Option<String>,
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
        Some(_) => unsupported(),
    }
}

/// `grant_type=client_credentials`, which is two shapes rather than one: with a
/// `guild_id` it answers with a row in `access_tokens`, and with a `scope` it
/// answers with a signed JWT. Neither is written yet — see docs/oauth2.md for
/// what each needs.
///
/// The credentials are read first because both shapes refuse a client that
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

    // In this order, as the Elixir's clauses are: a request that carries both a
    // guild and a scope gets the guild's answer.
    if let Some(guild_id) = form.guild_id {
        return granted_in_guild(state, &application, &guild_id).await;
    }

    if let Some(scope) = form.scope {
        return scoped(state, &application, &scope).await;
    }

    unsupported()
}

/// The shape that answers with a row: a token for a grant in a guild.
async fn granted_in_guild(state: &AppState, application: &Application, guild_id: &str) -> Response {
    let Ok(guild_id) = guild_id.parse::<i64>() else {
        return error("invalid_request", "guild_id");
    };

    let granted = grant_for(state.pool(), application.id, guild_id)
        .await
        .ok()
        .flatten();

    let Some(grant_id) = granted else {
        return invalid_client();
    };

    let now = OffsetDateTime::now_utc();

    let Ok(access_token) = create_access_token(state.pool(), grant_id, now).await else {
        return invalid_client();
    };

    let mut body = json!({
        "access_token": access_token,
        "token_type": "Bearer",
        // The constant here, where the Elixir subtracts the clock from the row it
        // has just written. The two agree to within the second it takes to say so.
        "expires_in": EXPIRES_IN,
    });

    if application
        .grant_types
        .iter()
        .any(|grant| grant == "refresh_token")
    {
        let Ok(refresh_token) = create_refresh_token(state.pool(), grant_id, now).await else {
            return invalid_client();
        };

        body["refresh_token"] = json!(refresh_token);
    }

    Json(body).into_response()
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
