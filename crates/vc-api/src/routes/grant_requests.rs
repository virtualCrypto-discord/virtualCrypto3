//! `/oauth2/clients/@me/grant-requests`: where an application asks for permission
//! — a guild for `vc.issue`, a person for the scopes that concern their own
//! account — the way a device asks in RFC 8628.
//!
//! Not the Elixir's. There the only way an application got a grant was a person
//! redeeming an authorization code in a browser, so an application had no call of
//! its own that could ask. This is that ask, for the flows that have no browser:
//! the row it writes is what a guild's or a person's yes answers, and that yes is
//! the grant the code flow would have written — after which the device poll
//! answers with the token the endpoint wants.
//!
//! Which of the two is being asked is the request's own body: one target, exactly,
//! because the answer it waits on belongs to one of them. `docs/issue.md` is the
//! guild's half and `docs/personal-grants.md` the person's.
//!
//! The token is the application's own: `Kind::App` with `oauth2.register`, which
//! is exactly the token registration answered with. A user token cannot ask for a
//! permission their application holds, because the caller's application is not
//! established by a user.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;

use vc_auth::AuthUser;
use vc_core::grant::Target;

use crate::routes::oauth2_clients::{Refusal, internal, refusal, refused};
use crate::state::AppState;

/// How long an ask lives when the application names none, in seconds:
/// ten minutes, which is RFC 8628's own recommendation for the device flow.
const DEFAULT_EXPIRES_IN: i64 = 600;

/// The longest an ask may live, in seconds: an hour, after which the guild's
/// screen would be showing an ask nobody remembers making.
const MAX_EXPIRES_IN: i64 = 3600;

/// What one request is: who is being asked — a guild's pool, or one person's own
/// account — the scopes it is asked for, the currencies it is for, and how long
/// the ask lives.
///
/// Exactly one of the two targets, which is also the database's rule: a body that
/// names both is not a request this service can file, because the answer it would
/// be waiting on is two different people's.
///
/// `resource` is RFC 8707's parameter, carried as an array because that is how a
/// JSON request carries the repeated form parameter the specification describes.
/// It is optional, and an ask that names none is an ask for everything — within
/// the target it already names, which is the whole of what it could mean. The
/// collection form says the same thing out loud.
#[derive(Deserialize)]
pub struct GrantRequest {
    pub guild_id: Option<String>,
    pub discord_id: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub resource: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "crate::json_number::deserialize_optional_i64"
    )]
    pub expires_in: Option<i64>,
}

impl GrantRequest {
    /// Which of the two the body named, and whether it named exactly one.
    fn target(&self) -> Result<Target, &'static str> {
        match (&self.guild_id, &self.discord_id) {
            (Some(guild), None) => guild
                .parse::<i64>()
                .map(Target::Guild)
                .map_err(|_| "the guild id must be a Discord id, as a string"),
            (None, Some(user)) => user
                .parse::<i64>()
                .map(Target::User)
                .map_err(|_| "the discord id must be a Discord id, as a string"),
            (Some(_), Some(_)) => Err("a request names a guild or a discord id, not both"),
            (None, None) => Err("a request names a guild or a discord id"),
        }
    }

    /// The scopes this target may be asked for. The two sets do not overlap, and
    /// the target is what says which one applies — asking a person for a guild's
    /// pool would be asking the wrong person, so it is refused here rather than
    /// approved into a grant nobody could use.
    fn checks(&self) -> fn(&[String]) -> Result<(), vc_core::application::ScopeError> {
        if self.discord_id.is_some() {
            vc_core::application::check_personal_scopes
        } else {
            vc_core::application::check_scopes
        }
    }
}

/// `POST /oauth2/clients/@me/grant-requests`: ask a guild to let this
/// application do what the scopes name.
///
/// The guild is named but never checked — it cannot be, because this service has
/// no proof the application belongs in any guild at all, and a wrong number in a
/// request is the guild's non-answer rather than this one's refusal. `guild_id` is
/// a string for the same reason every Discord id here is: a snowflake JSON's
/// number cannot hold.
///
/// The scopes are required, and they are checked the way the consent screen
/// checks its own: an approval grants exactly these, so an ask that names nothing
/// the service knows is refused here rather than approved into nothing.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<GrantRequest>,
) -> Response {
    let application = match own(&state, &user).await {
        Ok(application) => application,
        Err(refusal) => return refusal.response(),
    };

    let target = match body.target() {
        Ok(target) => target,
        Err(complaint) => {
            return refused(StatusCode::BAD_REQUEST, "invalid_request", complaint);
        }
    };

    let checks = body.checks();

    let Some(scopes) = body.scopes else {
        return refused(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "scopes are required",
        );
    };

    if checks(&scopes).is_err() {
        return refused(StatusCode::BAD_REQUEST, "invalid_scope", "unknown scope");
    }

    // What the ask is *for*, resolved the moment the scopes pass for the same
    // reason they are checked here: an approval grants exactly the ask, so a
    // target that is not a currency of this service — or, for a guild's ask, not
    // a currency of that guild — is refused here rather than approved into a
    // permission nobody can use.
    let resources = match crate::resource::resolve(
        state.pool(),
        &state.links().site_url,
        target,
        body.resource.as_deref().unwrap_or_default(),
    )
    .await
    {
        Ok(resources) => resources,
        Err(_) => return refused(StatusCode::BAD_REQUEST, "invalid_target", "invalid_target"),
    };

    let expires_in = match body.expires_in {
        None => DEFAULT_EXPIRES_IN,
        Some(expires_in) if expires_in > 0 && expires_in <= MAX_EXPIRES_IN => expires_in,
        Some(_) => {
            return refused(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "expires_in must be between 1 and 3600",
            );
        }
    };

    match vc_core::grant::request_grant(
        state.pool(),
        application,
        target,
        &scopes,
        &resources,
        expires_in,
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(asked) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "device_code": asked.device_code.to_string(),
                "user_code": asked.user_code,
                "verification_uri": "discord",
                "expires_in": asked.expires_in,
            })),
        )
            .into_response(),
        Err(_) => internal("the request could not be written").response(),
    }
}

/// `GET /oauth2/clients/@me/grant-requests`: what this application asked for,
/// answered or not — the application's own view of its asks, next to the poll
/// that spends them.
pub async fn index(State(state): State<AppState>, user: AuthUser) -> Response {
    let application = match own(&state, &user).await {
        Ok(application) => application,
        Err(refusal) => return refusal.response(),
    };

    match vc_core::grant::requests_of(state.pool(), application).await {
        Ok(requests) => Json(Value::Array(
            requests
                .into_iter()
                .map(|request| {
                    serde_json::json!({
                        "device_code": request.device_code.to_string(),
                        "user_code": request.user_code,
                        "guild_id": request.target.guild().map(|id| id.to_string()),
                        "discord_id": request.target.user().map(|id| id.to_string()),
                        "scopes": request.scopes,
                        "status": request.status,
                        "grant_id": request.grant_id.map(|id| id.to_string()),
                        "expires_in": request.expires_in,
                    })
                })
                .collect(),
        ))
        .into_response(),
        Err(_) => internal("the requests could not be read").response(),
    }
}

/// The application the token belongs to, which is also the whole of the
/// authorization question.
///
/// A user token asks with no application behind it — nothing says the caller *is*
/// the application a grant would be written for — so only the token issed at
/// registration time is let through. `oauth2.register` is required with it,
/// because asking a guild for a permission is application management, and that
/// token answered with that scope for exactly this.
async fn own(state: &AppState, user: &AuthUser) -> Result<i64, Refusal> {
    if user.kind != vc_auth::Kind::App {
        return Err(refusal(
            StatusCode::UNAUTHORIZED,
            "invalid_kind",
            "an application token is required",
        )
        .as_ref()
        .clone());
    }

    if !user.scopes.oauth2_register {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "oauth2.register is required",
        )
        .as_ref()
        .clone());
    }

    let Ok(subject) = i32::try_from(user.subject) else {
        return Err(internal("the token's subject is not an account id"));
    };

    match vc_core::user::application_id(state.pool(), subject).await {
        Ok(Some(application)) => Ok(application),
        Ok(None) => Err(internal("the application's account is gone")),
        Err(_) => Err(internal("the application could not be read")),
    }
}
