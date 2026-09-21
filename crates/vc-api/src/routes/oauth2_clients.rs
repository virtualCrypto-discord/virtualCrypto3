//! `GET/PATCH /oauth2/clients/@me`, and the shape an application is answered in.
//!
//! What is here is that shape: the sixteen fields and, more to the point, their
//! encodings. Three numbers travel as strings, the public key travels as
//! lowercase hex, and the secret's expiry is the literal zero that dynamic client
//! registration uses for "never". None of that can be inferred from the columns,
//! which is why it is written once and tested.
//!
//! The endpoints are here too: the read, the list, the registration and the edit.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use time::OffsetDateTime;
use vc_auth::AuthUser;

use vc_core::application::check_application_type as check_application_type_again;
use vc_core::application::{
    Changes, MetadataError, NewApplication, check_application_type, check_event_types,
    check_grant_types, check_logo_uri, check_response_types, check_slug, check_url,
};

use crate::discord_auth::user_profile;
use crate::error::ApiError;
use crate::notification::{Handshake, fresh_keypair};
use crate::rate_limit::TooSoon;
use crate::routes::v2::claims::format_timestamp;
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
    /// When the webhook last passed a handshake, and when it last failed one.
    ///
    /// `None` for an application that named no webhook, and for one whose webhook
    /// has not been re-checked since registration — the handshake at registration
    /// is not recorded, because it is the thing being established rather than
    /// something already true.
    pub webhook_verified_at: Option<time::PrimitiveDateTime>,
    pub webhook_failed_at: Option<time::PrimitiveDateTime>,
    /// The event types the application wants delivered, as the `type` values
    /// the delivery bodies carry. Checked is sent and unchecked is not, and
    /// empty is nothing.
    pub subscribed_events: Vec<i64>,
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
        // Whether the webhook is still answering, as the clock last found it.
        // `null` is "not checked since registration", which is the truth for
        // every application until the job reaches it.
        "webhook_verified_at": details.webhook_verified_at.map(format_timestamp),
        "webhook_failed_at": details.webhook_failed_at.map(format_timestamp),
        "subscribed_events": details.subscribed_events,
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
/// Three places in two queries. The join is on `users.application_id`, which links an
/// application to **the account created for it** — the one with no `discord_id`, made
/// by registration — and **not** to the person who registered it. The person is
/// `applications.owner_discord_id`, a Discord id rather than an account id, and
/// [`vc_core::application::owned_by`] is the read that goes through it.
///
/// This distinction is written out because it has already been got wrong once: the
/// list endpoint read this join as ownership and answered an empty list to somebody
/// who owned an application.
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
                  a.webhook_verified_at,
                  a.webhook_failed_at,
                  a.subscribed_events AS "subscribed_events!",
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
        webhook_verified_at: row.webhook_verified_at,
        webhook_failed_at: row.webhook_failed_at,
        subscribed_events: row.subscribed_events,
        public_key: row.public_key,
    }))
}

/// `GET /oauth2/clients/@me`, which is two answers to one path.
///
/// A **user** token's, and the applications whose `owner_discord_id` is that
/// person's Discord id — none, one, or several, because nothing makes it unique.
///
/// An `app` token is refused rather than answered: "the applications I own" is not a
/// question an application can ask about itself, and the Elixir answers it 401
/// `invalid_token` / `invalid_kind` (`clients_controller.ex` l.16-22).
///
/// Ownership is not `users.application_id`. That column is the other direction — it
/// points from the account created for an application to the application — and
/// reading it here gave a person with an application an empty list.
pub async fn mine(State(state): State<AppState>, user: AuthUser) -> Result<Response, ApiError> {
    if !user.scopes.oauth2_register {
        return Err(ApiError::PermissionDenied);
    }

    if user.kind != vc_auth::Kind::User {
        return Ok(refused(
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "invalid_kind",
        ));
    }

    let subject = i32::try_from(user.subject)
        .map_err(|_| ApiError::Internal("subject out of range".into()))?;

    // `ApiError` takes `vc_core::Error`, which is where a database error belongs:
    // the query is the domain's, and this is only the answering of it.
    let owned = vc_core::application::owned_by(state.pool(), subject)
        .await
        .map_err(vc_core::Error::from)?;

    let mut applications = Vec::with_capacity(owned.len());

    for application_id in owned {
        if let Some(found) = details(state.pool(), application_id)
            .await
            .map_err(vc_core::Error::from)?
        {
            applications.push(render(&found));
        }
    }

    Ok(Json(Value::Array(applications)).into_response())
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
    /// The event types the application wants delivered, as the `type` values
    /// the delivery bodies carry. Absent is everything — the default, which is
    /// what an application that never names a subscription gets.
    pub subscribed_events: Option<Vec<i64>>,
    /// The caller's Discord id, which the handler obtained by asking Discord about
    /// them — it is not taken from the request, and this is here to say so.
    #[serde(skip)]
    pub owner_discord_id: Option<i64>,
}

/// An OAuth error before it is a response.
///
/// The endpoint says it as JSON and the command surface reads the description out, so the
/// flows stop here and each surface talks. `description` is absent for an error that is
/// this service's fault: the reason goes to the log, and the caller is told it was ours.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub status: StatusCode,
    pub error: Cow<'static, str>,
    pub description: Option<Cow<'static, str>>,
}

impl Refusal {
    /// The JSON every endpoint around this one answers with.
    pub fn response(&self) -> Response {
        let mut body = json!({ "error": self.error.as_ref() });

        if let Some(description) = &self.description {
            body["error_description"] = json!(description.as_ref());
        }

        (self.status, Json(body)).into_response()
    }
}

/// A refusal, for a flow that returns one.
///
/// The sentences are anything a sentence can be made of, because some of them name what the
/// caller asked about: the bot that is not in the guild, the guild's own name.
pub fn refusal(
    status: StatusCode,
    error: impl Into<Cow<'static, str>>,
    description: impl Into<Cow<'static, str>>,
) -> Box<Refusal> {
    Box::new(Refusal {
        status,
        error: error.into(),
        description: Some(description.into()),
    })
}

/// An OAuth error, which is the shape every endpoint around this one answers with.
pub fn refused(
    status: StatusCode,
    error: impl Into<Cow<'static, str>>,
    description: impl Into<Cow<'static, str>>,
) -> Response {
    refusal(status, error, description).response()
}

/// A metadata refusal, which is always `invalid_client_metadata` and the rule's
/// own description.
fn metadata(error: MetadataError) -> Box<Refusal> {
    refusal(
        StatusCode::BAD_REQUEST,
        "invalid_client_metadata",
        error.description(),
    )
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

    validated(body).map_err(|refusal| Box::new(refusal.response()))
}

/// What was asked for, once it makes sense.
///
/// The metadata checks without the question of who is asking, because a registration
/// arrives from two places that answer that differently: an account's token, which says
/// which account is asking, and a form filled in beside a Discord message, where the
/// interaction's signature says it and there is no token at all. The auth is each
/// surface's and stays there.
pub fn validated(body: Registration) -> Result<NewApplication, Box<Refusal>> {
    let response_types =
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

    let subscribed_events = match body.subscribed_events.as_deref() {
        // Absent is everything, which is also what the column defaults to: an
        // application that never names a subscription gets every event.
        None => crate::developer::all_events().to_vec(),
        Some(types) => check_event_types(types).map_err(metadata)?,
    };

    let Some(redirect_uris) = body.redirect_uris else {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect_uris_must_be_array",
        ));
    };

    // Not `check_url`'s error: a redirect URI's refusal names the redirect URI
    // rather than the metadata, which is the pair the clients controller answers
    // with.
    validate_redirects(&redirect_uris)?;

    Ok(NewApplication {
        response_types,
        grant_types,
        application_type,
        client_name: body.client_name,
        client_uri: body.client_uri,
        logo_uri: body.logo_uri,
        webhook_url: body.webhook_url,
        discord_support_server_invite_slug: body.discord_support_server_invite_slug,
        owner_discord_id: body.owner_discord_id,
        redirect_uris,
        subscribed_events,
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

    /// A webhook that did not answer is the application's own URL being silent,
    /// and a proxy that did not answer is this service's way to applications
    /// failing: the same silence, and two different mistakes to go and look at.
    #[test]
    fn a_silence_is_the_applications_only_when_it_was_asked_directly() {
        let answered_wrongly = unverified(Handshake::Failed, true);

        assert_eq!(answered_wrongly.status, StatusCode::BAD_REQUEST);
        assert_eq!(answered_wrongly.error, "webhook_verification_failed");

        let direct = unverified(Handshake::Unreachable, false);

        assert_eq!(direct.status, StatusCode::BAD_REQUEST);
        assert_eq!(direct.error, "webhook_verification_failed");

        let through_a_proxy = unverified(Handshake::Unreachable, true);

        assert_eq!(through_a_proxy.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(through_a_proxy.error, "server_error");
        assert_eq!(
            through_a_proxy.description, None,
            "this service's own faults are not described to the caller"
        );
    }
}

fn validate_redirects(uris: &[String]) -> Result<(), Box<Refusal>> {
    use vc_core::application::{RedirectUriError, check_redirect_uris};
    check_redirect_uris(uris).map_err(|error| {
        refusal(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            match error {
                RedirectUriError::Scheme => "redirect_uri_scheme_must_be_http_or_https",
                RedirectUriError::TooLong => "redirect_uri_must_be_at_most_255_characters",
                RedirectUriError::ListTooLong => {
                    "redirect_uris_must_be_at_most_2000_characters_including_newlines"
                }
            },
        )
    })
}

/// `POST /oauth2/clients`: register an application.
///
/// The order is the Elixir's, and each step has its own refusal: the metadata,
/// then the owner's Discord profile (OAuth or bot lookup), then —
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
        return internal("the token's subject is not an account id").response();
    };

    let Some(account) = vc_core::user::find_by_id(state.pool(), subject)
        .await
        .ok()
        .flatten()
    else {
        return internal("the registering account is gone").response();
    };

    let Some(discord_id) = account.discord_id else {
        return internal("the registering account has no discord id").response();
    };

    let Ok(profile) = user_profile(&state, discord_id).await else {
        return internal("discord could not be asked about the account").response();
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

    new.owner_discord_id = Some(discord_id);

    let created = match create(&state, subject, &new).await {
        Ok(created) => created,
        Err(refusal) => return refusal.response(),
    };

    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": created.client_id,
            "client_secret": created.client_secret,
            "registration_access_token": created.access_token,
            "registration_client_uri": format!("{}/oauth2/clients/@me", state.links().site_url),
            "client_secret_expires_at": 0,
        })),
    )
        .into_response()
}

/// What a registration created: the three values both surfaces need.
///
/// The endpoint says them as JSON and the command says them as a screen, so this stops at
/// the values and lets each one talk.
pub struct Created {
    pub client_id: String,
    pub client_secret: String,
    pub access_token: String,
}

/// A registration, from where each surface can meet: after the caller is established as an
/// account, and before anything is said back.
///
/// The HTTP route establishes the caller with a token and then asks Discord who they are;
/// a Discord form has the interaction's signature for that. What is left is the same work
/// either way — the keypair, the webhook handshake, the row, and the registration token —
/// and it is here once.
///
/// `subject` is only the account the handshake's rate limit is charged to, which is the
/// account that would be asking twice.
pub async fn create(
    state: &AppState,
    subject: i32,
    new: &NewApplication,
) -> Result<Created, Box<Refusal>> {
    // Both halves, because the row needs both: the private one signs deliveries
    // and the public one is what the application verifies them with.
    let private_key = fresh_keypair();
    let public_key = ed25519_dalek::SigningKey::from_bytes(&private_key)
        .verifying_key()
        .to_bytes();

    if let Some(webhook_url) = new.webhook_url.as_deref() {
        // The handshake is the expensive thing here, and this is what stops one
        // requester spending all of it.
        if let Some(too_soon) = state.handshake_limiter().refuse(&subject.to_string()) {
            return Err(Box::new(rate_limited(too_soon)));
        }

        let (handshake, through_proxy) =
            crate::notification::check_webhook(state, webhook_url, &private_key).await;

        if handshake != Handshake::Passed {
            return Err(unverified(handshake, through_proxy));
        }
    }

    let registered =
        match vc_core::application::register(state.pool(), new, &private_key, &public_key).await {
            Ok(registered) => registered,
            Err(_) => return Err(Box::new(internal("the application could not be written"))),
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
        return Err(Box::new(internal(
            "the registration token could not be issued",
        )));
    };

    Ok(Created {
        client_id: registered.client_id,
        client_secret: registered.client_secret,
        access_token,
    })
}

/// A handshake that came too soon.
///
/// The three windows answer different questions — a retry storm, somebody
/// hammering, a day's worth of this service's time — so a client is told which one
/// it hit rather than only that it hit one.
fn rate_limited(too_soon: TooSoon) -> Refusal {
    let description = match too_soon {
        TooSoon::Seconds3 => "retry_after_3_seconds",
        TooSoon::Hour => "retry_after_1_hour",
        TooSoon::Day => "retry_after_1_day",
    };

    Refusal {
        status: StatusCode::TOO_MANY_REQUESTS,
        error: "rate_limit_exceeded".into(),
        description: Some(description.into()),
    }
}

/// What a handshake that did not pass is answered as.
///
/// A webhook that answered wrongly is the application's to fix, and so is one that
/// did not answer at all when this service asked it **directly**: the URL the
/// application named is what is silent.
///
/// A silence through the proxy is not the application's. That worker is this
/// service's own way of reaching applications, so failing to reach it is a
/// deployment this service cannot deliver through — and answering it as
/// `webhook_verification_failed` would send the application's owner to look at the
/// one place the problem is not.
fn unverified(handshake: Handshake, through_proxy: bool) -> Box<Refusal> {
    if handshake == Handshake::Unreachable && through_proxy {
        return Box::new(internal("the webhook proxy did not answer"));
    }

    refusal(
        StatusCode::BAD_REQUEST,
        "webhook_verification_failed",
        "the webhook did not verify",
    )
}

/// A refusal that is this service's fault rather than the caller's.
///
/// A registration can fail because Discord is unreachable, because the database
/// will not take the write, or because the webhook proxy it verifies through did
/// not answer — and none of those is something the caller can act on, so none of
/// them is answered as though the request were wrong.
pub fn internal(why: &'static str) -> Refusal {
    tracing::error!(why, "a registration could not be completed");

    Refusal {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        error: "server_error".into(),
        description: None,
    }
}

/// What a `PATCH` asks to change, with each field's presence intact.
///
/// A `serde` option cannot tell "absent" from "null" — both arrive as `None` — and
/// in this endpoint they are different operations: sending `logo_uri: null` clears
/// it, and not mentioning it leaves it alone. So the body is read as a map and each
/// field is asked for rather than deserialised.
pub fn changes(body: &Map<String, Value>) -> Result<Changes, Box<Refusal>> {
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

    fn events(body: &Map<String, Value>) -> Option<Vec<i64>> {
        body.get("subscribed_events")?.as_array().map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_i64()
                        .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
                })
                .collect::<Option<Vec<_>>>()
        })?
    }

    let rotate_client_secret = match body.get("client_secret") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(_) => {
            return Err(refusal(
                StatusCode::BAD_REQUEST,
                "invalid_client_metadata",
                "client_secret_must_be_boolean_or_not_set",
            ));
        }
    };

    let changes = Changes {
        rotate_client_secret,
        client_name: text(body, "client_name"),
        client_uri: text(body, "client_uri"),
        logo_uri: text(body, "logo_uri"),
        webhook_url: text(body, "webhook_url"),
        discord_support_server_invite_slug: text(body, "discord_support_server_invite_slug"),
        application_type: text(body, "application_type").flatten(),
        grant_types: list(body, "grant_types"),
        response_types: list(body, "response_types"),
        redirect_uris: list(body, "redirect_uris"),
        subscribed_events: events(body),
    };

    // A value that is not a number is not a value the service knows: the menu
    // sends what it was given, so a request naming one is refused here rather
    // than stored as if it had named nothing.
    if body.get("subscribed_events").is_some() && changes.subscribed_events.is_none() {
        return Err(metadata(MetadataError::Events));
    }

    if let Some(types) = changes.subscribed_events.as_deref() {
        check_event_types(types).map_err(metadata)?;
    }

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
        validate_redirects(uris)?;
    }

    Ok(changes)
}

/// `PATCH /oauth2/clients/@me`: edit the application the token is for.
///
/// An `app` token, because the application is asking about itself — and nothing
/// is written for the fields the request does not name, which is what `changes`
/// preserves and `patch` honours.
/// `GET /oauth2/clients/@me`: the application the token is for.
///
/// This is RFC 7592's read of the client identified by the token, and it is the URI
/// `POST /oauth2/clients` names in `registration_client_uri` — so it takes an
/// **application** token, the one that registration answered with, and a user token is
/// refused with the same `invalid_kind` that `PATCH` on this path gives. The two being
/// the same call is the point: a client that follows the registration's own answer has
/// to be able to read itself at the address it was handed.
pub async fn me(State(state): State<AppState>, user: AuthUser) -> Response {
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

    let Ok(subject) = i32::try_from(user.subject) else {
        return internal("the token's subject is not an account id").response();
    };

    let found = vc_core::user::application_id(state.pool(), subject)
        .await
        .ok()
        .flatten();

    let Some(application_id) = found else {
        return internal("the application's account is gone").response();
    };

    match details(state.pool(), application_id).await {
        Ok(Some(found)) => Json(render(&found)).into_response(),
        Ok(None) => refused(StatusCode::NOT_FOUND, "not_found", "no such application"),
        Err(_) => internal("the application could not be read").response(),
    }
}

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
        Err(refusal) => return refusal.response(),
    };

    let Ok(subject) = i32::try_from(user.subject) else {
        return internal("the token's subject is not an account id").response();
    };

    let found = vc_core::user::application_id(state.pool(), subject)
        .await
        .ok()
        .flatten();

    let Some(application_id) = found else {
        return internal("the application's account is gone").response();
    };

    match apply(&state, application_id, &subject.to_string(), &changes).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(refusal) => refusal.response(),
    }
}

/// A `PATCH`, from where both surfaces can meet: after the caller is established, the body
/// has been read, and there is an application to write to.
///
/// `key` is the account the handshake's rate limit is charged to. The endpoint passes the
/// token's subject, which is the application's own account, and so does the command — the
/// budget belongs to the application rather than to whichever surface asked.
pub async fn apply(
    state: &AppState,
    application_id: i64,
    key: &str,
    changes: &Changes,
) -> Result<(), Box<Refusal>> {
    // A named webhook is verified with the key the application **already** has, not
    // with a fresh pair: the pair is what it verifies deliveries with, so a new one
    // would be a handshake nobody could answer.
    if let Some(Some(webhook_url)) = changes.webhook_url.as_ref() {
        let webhook = vc_core::application::webhook_data(state.pool(), application_id)
            .await
            .ok()
            .flatten();

        let Some(webhook) = webhook else {
            return Err(Box::new(internal("the application has no webhook data")));
        };

        // The Elixir validator accepts the stored URL without another handshake.
        // An unchanged URL also leaves the verification budget untouched.
        if webhook.webhook_url.as_deref() != Some(webhook_url.as_str()) {
            let Ok(private_key) = <[u8; 32]>::try_from(webhook.private_key.as_slice()) else {
                return Err(Box::new(internal(
                    "the application's private key is not 32 bytes",
                )));
            };

            // The handshake is the expensive thing here, and this is what stops one
            // requester spending all of it.
            if let Some(too_soon) = state.handshake_limiter().refuse(key) {
                return Err(Box::new(rate_limited(too_soon)));
            }

            let (handshake, through_proxy) =
                crate::notification::check_webhook(state, webhook_url, &private_key).await;

            if handshake != Handshake::Passed {
                return Err(unverified(handshake, through_proxy));
            }
        }
    }

    if vc_core::application::patch(state.pool(), application_id, changes)
        .await
        .is_err()
    {
        return Err(Box::new(internal("the application could not be written")));
    }

    Ok(())
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
            webhook_verified_at: None,
            webhook_failed_at: None,
            subscribed_events: vec![2, 3],
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
    fn all_seventeen_fields_are_present() {
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
            "subscribed_events",
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
