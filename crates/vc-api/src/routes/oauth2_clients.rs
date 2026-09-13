//! `GET/PATCH /oauth2/clients/@me`, and the shape an application is answered in.
//!
//! What is here is that shape: the sixteen fields and, more to the point, their
//! encodings. Three numbers travel as strings, the public key travels as
//! lowercase hex, and the secret's expiry is the literal zero that dynamic client
//! registration uses for "never". None of that can be inferred from the columns,
//! which is why it is written once and tested.
//!
//! The endpoints themselves are not here yet: they need the read that fills this
//! in, and the registration that issues a client secret.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use vc_auth::AuthUser;

use vc_core::application::check_application_type as check_application_type_again;
use vc_core::application::{
    Changes, MetadataError, NewApplication, check_application_type, check_grant_types,
    check_logo_uri, check_response_types, check_slug, check_url,
};

use crate::discord_auth::resolve_token;
use crate::error::ApiError;
use crate::notification::{Handshake, fresh_keypair, verify};
use crate::rate_limit::TooSoon;
use crate::state::AppState;

/// An application as the API answers it, with its owner and its redirect URIs.
///
/// Named for what it is rather than for the table it comes from: the record the
/// endpoints answer with, gathered from three places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Details {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uris: Vec<String>,
    /// The owning account.
    pub user_id: i32,
    pub discord_user_id: Option<i64>,
    pub application_type: String,
    pub client_name: Option<String>,
    pub client_uri: Option<String>,
    pub discord_support_server_invite_slug: Option<String>,
    pub grant_types: Vec<String>,
    pub logo_uri: Option<String>,
    pub owner_discord_id: Option<i64>,
    pub response_types: Vec<String>,
    pub webhook_url: Option<String>,
    /// The application's public key, as raw bytes — the rendering is what makes
    /// it hex, and doing it here would make the hex a thing two places know.
    pub public_key: Vec<u8>,
}

/// `Clients.render_application/1`.
///
/// The single read, the list and the registration all answer with this, so an
/// error here is an error in three endpoints at once.
pub fn render(details: &Details) -> Value {
    json!({
        "client_id": details.client_id,
        "client_secret": details.client_secret,
        // The convention for "this secret never expires", which is a zero rather
        // than a null so that a client reading an integer gets one.
        "client_secret_expires_at": 0,
        "redirect_uris": details.redirect_uris,
        "user_id": details.user_id.to_string(),
        "discord_user_id": details
            .discord_user_id
            .map(|discord_id| discord_id.to_string()),
        "application_type": details.application_type,
        "client_name": details.client_name,
        "client_uri": details.client_uri,
        "discord_support_server_invite_slug": details.discord_support_server_invite_slug,
        "grant_types": details.grant_types,
        "logo_uri": details.logo_uri,
        "owner_discord_id": details
            .owner_discord_id
            .map(|discord_id| discord_id.to_string()),
        "response_types": details.response_types,
        "webhook_url": details.webhook_url,
        // Lowercase hex, and the same spelling as the delivery signature — an
        // application holds this and compares it with what it registered.
        "public_key": details
            .public_key
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    })
}

/// The read that fills [`Details`] in: the application, the account that owns it,
/// and its redirect URIs.
///
/// Three places in two queries. The join is on `users.application_id`, which is
/// the only link between an application and the account that registered it.
pub async fn details(
    pool: &sqlx::PgPool,
    application_id: i64,
) -> std::result::Result<Option<Details>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT a.client_id::text AS "client_id!",
                  a.client_secret,
                  a.application_type::text AS "application_type!",
                  a.client_name,
                  a.client_uri,
                  a.discord_support_server_invite_slug,
                  a.grant_types::text[] AS "grant_types!",
                  a.logo_uri,
                  a.owner_discord_id,
                  a.response_types::text[] AS "response_types!",
                  a.webhook_url,
                  a.public_key,
                  u.id AS user_id,
                  u.discord_id AS user_discord_id
             FROM applications a
             JOIN users u ON u.application_id = a.id
            WHERE a.id = $1"#,
        application_id
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let redirect_uris = sqlx::query_scalar!(
        r#"SELECT redirect_uri AS "redirect_uri!" FROM redirect_uris
            WHERE application_id = $1"#,
        application_id
    )
    .fetch_all(pool)
    .await?;

    Ok(Some(Details {
        client_id: row.client_id,
        client_secret: row.client_secret,
        redirect_uris,
        user_id: row.user_id,
        discord_user_id: row.user_discord_id,
        application_type: row.application_type,
        client_name: row.client_name,
        client_uri: row.client_uri,
        discord_support_server_invite_slug: row.discord_support_server_invite_slug,
        grant_types: row.grant_types,
        logo_uri: row.logo_uri,
        owner_discord_id: row.owner_discord_id,
        response_types: row.response_types,
        webhook_url: row.webhook_url,
        public_key: row.public_key,
    }))
}

/// `GET /oauth2/clients/@me`, which is two answers to one path.
///
/// A `user` token's subject is a person, and the answer is the applications that
/// account owns — an array of none or one, since `users.application_id` links an
/// account to at most one application. An `app` token's subject is the
/// application's own account, and the answer is that application alone.
///
/// Both find it the same way, so the difference is the shape of the answer rather
/// than the question.
pub async fn mine(State(state): State<AppState>, user: AuthUser) -> Result<Json<Value>, ApiError> {
    if !user.scopes.oauth2_register {
        return Err(ApiError::PermissionDenied);
    }

    let subject = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    // `ApiError` takes `vc_core::Error`, which is where a database error belongs:
    // the query is the domain's, and this is only the answering of it.
    let owned = vc_core::user::application_id(state.pool(), subject)
        .await
        .map_err(vc_core::Error::from)?;

    let found = match owned {
        Some(application_id) => details(state.pool(), application_id)
            .await
            .map_err(vc_core::Error::from)?,
        None => None,
    };

    Ok(Json(match user.kind {
        vc_auth::Kind::App => match &found {
            Some(found) => render(found),
            // An application token whose application is gone: nothing to say
            // about it, and the token is still a token.
            None => Value::Null,
        },
        vc_auth::Kind::User => Value::Array(found.iter().map(render).collect()),
    }))
}

/// A registration request, with nothing required: the Elixir answers a malformed
/// one with an OAuth error rather than a 422, so absence has to reach the checks
/// instead of being rejected on the way in.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Registration {
    pub response_types: Option<Vec<String>>,
    pub grant_types: Option<Vec<String>>,
    pub application_type: Option<String>,
    pub client_name: Option<String>,
    pub client_uri: Option<String>,
    pub logo_uri: Option<String>,
    pub webhook_url: Option<String>,
    pub discord_support_server_invite_slug: Option<String>,
    pub redirect_uris: Option<Vec<String>>,
    /// The caller's Discord id, which the handler obtained by asking Discord about
    /// them — it is not taken from the request, and this is here to say so.
    #[serde(skip)]
    pub owner_discord_id: Option<i64>,
}

/// An OAuth error, which is the shape every endpoint around this one answers with.
fn refused(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

/// A metadata refusal, which is always `invalid_client_metadata` and the rule's
/// own description.
fn metadata(error: MetadataError) -> Box<Response> {
    Box::new(refused(
        StatusCode::BAD_REQUEST,
        "invalid_client_metadata",
        error.description(),
    ))
}

/// The checks that come before anything is asked of Discord: who is asking,
/// whether they may, and whether what they asked for makes sense.
///
/// The order is the Elixir's and is observable, because the first failure is the
/// one reported. The two defaults are the Elixir's too: an absent
/// `application_type` is `web`, and absent `grant_types` is an **empty list** —
/// not the column's default — so a registration that asks for nothing gets an
/// application that can do nothing.
pub fn checked(user: &AuthUser, body: Registration) -> Result<NewApplication, Box<Response>> {
    if user.kind != vc_auth::Kind::User {
        return Err(Box::new(refused(
            StatusCode::UNAUTHORIZED,
            "invalid_kind",
            "a user token is required",
        )));
    }

    if !user.scopes.oauth2_register {
        return Err(Box::new(refused(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "oauth2.register is required",
        )));
    }

    check_response_types(&body.response_types.unwrap_or_default()).map_err(metadata)?;
    let grant_types = check_grant_types(&body.grant_types.unwrap_or_default()).map_err(metadata)?;

    let application_type = body.application_type.unwrap_or_else(|| "web".to_owned());
    check_application_type(&application_type).map_err(metadata)?;

    // `client_name` is taken as it comes: the Elixir's validator for it is
    // `&{:ok, &1}`, which is to say there is none.
    if let Some(client_uri) = body.client_uri.as_deref() {
        check_url(client_uri, MetadataError::ClientUri).map_err(metadata)?;
    }

    if let Some(logo_uri) = body.logo_uri.as_deref() {
        check_logo_uri(logo_uri).map_err(metadata)?;
    }

    if let Some(webhook_url) = body.webhook_url.as_deref() {
        check_url(webhook_url, MetadataError::WebhookUrl).map_err(metadata)?;
    }

    if let Some(slug) = body.discord_support_server_invite_slug.as_deref() {
        check_slug(slug).map_err(metadata)?;
    }

    let Some(redirect_uris) = body.redirect_uris else {
        return Err(Box::new(refused(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect_uris_must_be_array",
        )));
    };

    // Not `check_url`'s error: a redirect URI's refusal names the redirect URI
    // rather than the metadata, which is the pair the clients controller answers
    // with.
    for redirect_uri in &redirect_uris {
        if check_url(redirect_uri, MetadataError::ClientUri).is_err() {
            return Err(Box::new(refused(
                StatusCode::BAD_REQUEST,
                "invalid_redirect_uri",
                "redirect_uri_scheme_must_be_http_or_https",
            )));
        }
    }

    Ok(NewApplication {
        grant_types,
        application_type,
        client_name: body.client_name,
        client_uri: body.client_uri,
        logo_uri: body.logo_uri,
        webhook_url: body.webhook_url,
        discord_support_server_invite_slug: body.discord_support_server_invite_slug,
        owner_discord_id: body.owner_discord_id,
        redirect_uris,
    })
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    use vc_auth::Scopes;

    fn user(kind: vc_auth::Kind, oauth2_register: bool) -> AuthUser {
        AuthUser {
            subject: 7,
            kind,
            scopes: Scopes {
                oauth2_register,
                ..Scopes::default()
            },
            jti: String::new(),
        }
    }

    fn body() -> Registration {
        Registration {
            grant_types: Some(vec!["authorization_code".to_owned()]),
            redirect_uris: Some(vec!["https://app.example/callback".to_owned()]),
            ..Registration::default()
        }
    }

    fn status(response: &Response) -> u16 {
        response.status().as_u16()
    }

    #[test]
    fn a_user_with_the_scope_is_let_through() {
        let checked = checked(&user(vc_auth::Kind::User, true), body());

        assert!(checked.is_ok(), "{:?}", checked.err().map(|r| r.status()));
    }

    /// A user token is what registration is for; an application cannot register
    /// another.
    #[test]
    fn an_application_token_is_refused() {
        let response = checked(&user(vc_auth::Kind::App, true), body()).expect_err("a refusal");

        assert_eq!(status(&response), 401);
    }

    #[test]
    fn a_user_without_the_scope_is_refused() {
        let response = checked(&user(vc_auth::Kind::User, false), body()).expect_err("a refusal");

        assert_eq!(status(&response), 403);
    }

    /// The two defaults, and the second is the one that bites: asking for no grant
    /// types gives an application that can do nothing.
    #[test]
    fn absent_fields_take_the_elixirs_defaults() {
        let checked = checked(
            &user(vc_auth::Kind::User, true),
            Registration {
                redirect_uris: Some(vec!["https://app.example/callback".to_owned()]),
                ..Registration::default()
            },
        )
        .expect("a registration");

        assert_eq!(checked.application_type, "web");
        assert!(checked.grant_types.is_empty(), "{:?}", checked.grant_types);
    }

    #[test]
    fn a_redirect_uri_that_is_not_http_is_refused_by_name() {
        let response = checked(
            &user(vc_auth::Kind::User, true),
            Registration {
                redirect_uris: Some(vec!["ftp://app.example".to_owned()]),
                ..body()
            },
        )
        .expect_err("a refusal");

        assert_eq!(status(&response), 400);
    }

    /// The order is the contract: this request is wrong about two things and is
    /// refused for the metadata rather than for the redirect URI.
    #[test]
    fn the_first_failure_is_the_one_reported() {
        let response = checked(
            &user(vc_auth::Kind::User, true),
            Registration {
                application_type: Some("service".to_owned()),
                redirect_uris: Some(vec!["ftp://app.example".to_owned()]),
                ..Registration::default()
            },
        )
        .expect_err("a refusal");

        assert_eq!(status(&response), 400);
    }
}

/// `POST /oauth2/clients`: register an application.
///
/// The order is the Elixir's, and each step has its own refusal: the metadata,
/// then the owner's Discord authorization, then who Discord says they are, then —
/// only if a webhook was given — whether it verifies, and then the writing.
pub async fn register(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<Registration>,
) -> Response {
    let mut new = match checked(&user, body) {
        Ok(new) => new,
        Err(refusal) => return *refusal,
    };

    let Some(subject) = i32::try_from(user.subject).ok() else {
        return internal("the token's subject is not an account id");
    };

    let Some(account) = vc_core::user::find_by_id(state.pool(), subject)
        .await
        .ok()
        .flatten()
    else {
        return internal("the registering account is gone");
    };

    let Some(discord_id) = account.discord_id else {
        return internal("the registering account has no discord id");
    };

    let Some(authorization) = vc_core::user::find_discord_auth(state.pool(), discord_id)
        .await
        .ok()
        .flatten()
    else {
        return internal("the registering account has no authorization");
    };

    let Ok(token) = resolve_token(&state, discord_id, &authorization).await else {
        return internal("the authorization could not be refreshed");
    };

    let Ok(profile) = state.discord().get_user_info(&token).await else {
        return internal("discord could not be asked about the account");
    };

    // A bot account may not register: an application registering applications is
    // not what this endpoint is for.
    if profile.get("bot").and_then(Value::as_bool).unwrap_or(false) {
        return refused(
            StatusCode::BAD_REQUEST,
            "user_verification_failed",
            "a bot account may not register",
        );
    }

    new.owner_discord_id = profile
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse().ok());

    // Both halves, because the row needs both: the private one signs deliveries
    // and the public one is what the application verifies them with.
    let private_key = fresh_keypair();
    let public_key = ed25519_dalek::SigningKey::from_bytes(&private_key)
        .verifying_key()
        .to_bytes();

    if let Some(webhook_url) = new.webhook_url.as_deref() {
        let Some(proxy) = state.webhook_proxy() else {
            // Not the application's fault, and saying "verification failed" would
            // send someone to look at a request that is fine.
            return internal("no webhook proxy is configured");
        };

        // The handshake is the expensive thing here, and this is what stops one
        // requester spending all of it.
        if let Some(too_soon) = state.handshake_limiter().refuse(&subject.to_string()) {
            return rate_limited(too_soon);
        }

        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs() as i64)
            .unwrap_or_default();

        if verify(proxy, webhook_url, &private_key, at).await != Handshake::Passed {
            return refused(
                StatusCode::BAD_REQUEST,
                "webhook_verification_failed",
                "the webhook did not verify",
            );
        }
    }

    let registered =
        match vc_core::application::register(state.pool(), &new, &private_key, &public_key).await {
            Ok(registered) => registered,
            Err(_) => return internal("the application could not be written"),
        };

    // The registration access token is issued for the account registration
    // created — the application rather than the person who registered it.
    let issued = vc_auth::issue::app_token(
        state.pool(),
        state.jwt_secret(),
        i64::from(registered.user_id),
        &["oauth2.register"],
        OffsetDateTime::now_utc(),
    )
    .await;

    let Ok(access_token) = issued else {
        return internal("the registration token could not be issued");
    };

    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": registered.client_id,
            "client_secret": registered.client_secret,
            "registration_access_token": access_token,
            "registration_client_uri": format!("{}/oauth2/clients/@me", state.links().site_url),
            "client_secret_expires_at": 0,
        })),
    )
        .into_response()
}

/// A handshake that came too soon.
///
/// The three windows answer different questions — a retry storm, somebody
/// hammering, a day's worth of this service's time — so a client is told which one
/// it hit rather than only that it hit one.
fn rate_limited(too_soon: TooSoon) -> Response {
    let description = match too_soon {
        TooSoon::Seconds3 => "retry_after_3_seconds",
        TooSoon::Hour => "retry_after_1_hour",
        TooSoon::Day => "retry_after_1_day",
    };

    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": "rate_limit_exceeded", "error_description": description })),
    )
        .into_response()
}

/// A refusal that is this service's fault rather than the caller's.
///
/// A registration can fail because Discord is unreachable, because the database
/// will not take the write, or because this service has no proxy — and none of
/// those is something the caller can act on, so none of them is answered as though
/// the request were wrong.
fn internal(why: &str) -> Response {
    tracing::error!(why, "a registration could not be completed");

    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "server_error" })),
    )
        .into_response()
}

/// What a `PATCH` asks to change, with each field's presence intact.
///
/// A `serde` option cannot tell "absent" from "null" — both arrive as `None` — and
/// in this endpoint they are different operations: sending `logo_uri: null` clears
/// it, and not mentioning it leaves it alone. So the body is read as a map and each
/// field is asked for rather than deserialised.
pub fn changes(body: &Map<String, Value>) -> Result<Changes, Box<Response>> {
    /// `None` when the field is not in the request, `Some(None)` when it is null,
    /// and `Some(Some(..))` when it has a value.
    fn text(body: &Map<String, Value>, field: &str) -> Option<Option<String>> {
        match body.get(field)? {
            Value::Null => Some(None),
            // A number or a boolean is answered as its JSON text, which is what a
            // database column would have been given; refusing it here would be a
            // difference nobody asked for.
            other => Some(Some(
                other
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| other.to_string()),
            )),
        }
    }

    fn list(body: &Map<String, Value>, field: &str) -> Option<Vec<String>> {
        body.get(field)?.as_array().map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                })
                .collect()
        })
    }

    let changes = Changes {
        client_name: text(body, "client_name"),
        client_uri: text(body, "client_uri"),
        logo_uri: text(body, "logo_uri"),
        webhook_url: text(body, "webhook_url"),
        discord_support_server_invite_slug: text(body, "discord_support_server_invite_slug"),
        application_type: text(body, "application_type").flatten(),
        grant_types: list(body, "grant_types"),
        response_types: list(body, "response_types"),
        redirect_uris: list(body, "redirect_uris"),
    };

    // The same rules registration applies, to the fields that are here: an edit is
    // a registration of the parts it names.
    if let Some(kind) = changes.application_type.as_deref() {
        check_application_type_again(kind).map_err(metadata)?;
    }

    if let Some(Some(client_uri)) = changes.client_uri.as_ref() {
        check_url(client_uri, MetadataError::ClientUri).map_err(metadata)?;
    }

    if let Some(Some(logo_uri)) = changes.logo_uri.as_ref() {
        check_logo_uri(logo_uri).map_err(metadata)?;
    }

    if let Some(Some(webhook_url)) = changes.webhook_url.as_ref() {
        check_url(webhook_url, MetadataError::WebhookUrl).map_err(metadata)?;
    }

    if let Some(Some(slug)) = changes.discord_support_server_invite_slug.as_ref() {
        check_slug(slug).map_err(metadata)?;
    }

    if let Some(types) = changes.grant_types.as_deref() {
        check_grant_types(types).map_err(metadata)?;
    }

    if let Some(types) = changes.response_types.as_deref() {
        check_response_types(types).map_err(metadata)?;
    }

    if let Some(uris) = changes.redirect_uris.as_deref() {
        for redirect_uri in uris {
            if check_url(redirect_uri, MetadataError::ClientUri).is_err() {
                return Err(Box::new(refused(
                    StatusCode::BAD_REQUEST,
                    "invalid_redirect_uri",
                    "redirect_uri_scheme_must_be_http_or_https",
                )));
            }
        }
    }

    Ok(changes)
}

/// `PATCH /oauth2/clients/@me`: edit the application the token is for.
///
/// An `app` token, because the application is asking about itself — and nothing
/// is written for the fields the request does not name, which is what `changes`
/// preserves and `patch` honours.
pub async fn edit(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<Map<String, Value>>,
) -> Response {
    if user.kind != vc_auth::Kind::App {
        return refused(
            StatusCode::UNAUTHORIZED,
            "invalid_kind",
            "an application token is required",
        );
    }

    if !user.scopes.oauth2_register {
        return refused(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "oauth2.register is required",
        );
    }

    let changes = match changes(&body) {
        Ok(changes) => changes,
        Err(refusal) => return *refusal,
    };

    let Ok(subject) = i32::try_from(user.subject) else {
        return internal("the token's subject is not an account id");
    };

    let found = vc_core::user::application_id(state.pool(), subject)
        .await
        .ok()
        .flatten();

    let Some(application_id) = found else {
        return internal("the application's account is gone");
    };

    // A named webhook is verified with the key the application **already** has, not
    // with a fresh pair: the pair is what it verifies deliveries with, so a new one
    // would be a handshake nobody could answer.
    if let Some(Some(webhook_url)) = changes.webhook_url.as_ref() {
        let Some(proxy) = state.webhook_proxy() else {
            return internal("no webhook proxy is configured");
        };

        let webhook = vc_core::application::webhook_data(state.pool(), application_id)
            .await
            .ok()
            .flatten();

        let Some(webhook) = webhook else {
            return internal("the application has no webhook data");
        };

        let Ok(private_key) = <[u8; 32]>::try_from(webhook.private_key.as_slice()) else {
            return internal("the application's private key is not 32 bytes");
        };

        // The handshake is the expensive thing here, and this is what stops one
        // requester spending all of it.
        if let Some(too_soon) = state.handshake_limiter().refuse(&subject.to_string()) {
            return rate_limited(too_soon);
        }

        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs() as i64)
            .unwrap_or_default();

        if verify(proxy, webhook_url, &private_key, at).await != Handshake::Passed {
            return refused(
                StatusCode::BAD_REQUEST,
                "webhook_verification_failed",
                "the webhook did not verify",
            );
        }
    }

    if vc_core::application::patch(state.pool(), application_id, &changes)
        .await
        .is_err()
    {
        return internal("the application could not be written");
    }

    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn details() -> Details {
        Details {
            client_id: "e0e4a8ce-6d0e-4a5e-9f4a-1a2b3c4d5e6f".to_owned(),
            client_secret: Some("a-secret".to_owned()),
            redirect_uris: vec!["https://app.example/callback".to_owned()],
            user_id: 7,
            discord_user_id: Some(100_000_000_000_000_001),
            application_type: "web".to_owned(),
            client_name: Some("An Application".to_owned()),
            client_uri: None,
            discord_support_server_invite_slug: None,
            grant_types: vec!["authorization_code".to_owned()],
            logo_uri: None,
            owner_discord_id: Some(100_000_000_000_000_002),
            response_types: vec!["code".to_owned()],
            webhook_url: Some("https://app.example/hook".to_owned()),
            public_key: vec![0x00, 0xab, 0xff],
        }
    }

    /// The three numbers that are strings, which is the thing a client parses and
    /// therefore the thing that is easy to get wrong.
    #[test]
    fn the_numbers_that_are_strings_are_strings() {
        let rendered = render(&details());

        assert_eq!(rendered["user_id"], "7");
        assert_eq!(rendered["discord_user_id"], "100000000000000001");
        assert_eq!(rendered["owner_discord_id"], "100000000000000002");
        assert!(rendered["user_id"].is_string());
    }

    /// A null rather than a missing key: `unless ... do ... else nil end` keeps
    /// the field and nulls it.
    #[test]
    fn an_account_without_a_discord_id_is_null_rather_than_absent() {
        let mut details = details();
        details.discord_user_id = None;

        let rendered = render(&details);

        assert!(rendered.get("discord_user_id").is_some());
        assert_eq!(rendered["discord_user_id"], Value::Null);
    }

    /// Lowercase, because that is what an application would have registered.
    #[test]
    fn the_public_key_is_lowercase_hex() {
        let rendered = render(&details());

        assert_eq!(rendered["public_key"], "00abff");
    }

    /// The secret's expiry is a zero and never a null, which is the convention
    /// for a secret that does not expire rather than a missing value.
    #[test]
    fn the_secret_never_expires_as_a_zero() {
        let rendered = render(&details());

        assert_eq!(rendered["client_secret_expires_at"], 0);
        assert!(!rendered["client_secret_expires_at"].is_null());
    }

    /// Every field the shape promises, so that a misspelling is a failure here
    /// rather than three endpoints disagreeing about a name.
    #[test]
    fn all_sixteen_fields_are_present() {
        let rendered = render(&details());

        for field in [
            "client_id",
            "client_secret",
            "client_secret_expires_at",
            "redirect_uris",
            "user_id",
            "discord_user_id",
            "application_type",
            "client_name",
            "client_uri",
            "discord_support_server_invite_slug",
            "grant_types",
            "logo_uri",
            "owner_discord_id",
            "response_types",
            "webhook_url",
            "public_key",
        ] {
            assert!(rendered.get(field).is_some(), "{field} is missing");
        }
    }

    /// Absent values travel as nulls rather than being dropped, which is what the
    /// Elixir's map does.
    #[test]
    fn absent_optional_values_travel_as_nulls() {
        let rendered = render(&details());

        assert_eq!(rendered["client_uri"], Value::Null);
        assert_eq!(rendered["logo_uri"], Value::Null);
        assert_eq!(rendered["discord_support_server_invite_slug"], Value::Null);
    }
}
