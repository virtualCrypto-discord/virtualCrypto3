//! The consent screen.
//!
//! Validate the authorization request, show the browser an approval form, and
//! repeat the checks on submission before issuing a code.

mod consent;
mod flow;

use axum::extract::{Query, RawQuery, State};
use axum::http::header::{CACHE_CONTROL, REFERRER_POLICY, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Json};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::ApiError;
use crate::state::AppState;
use uuid::Uuid;
use vc_core::application::PreauthorizeError;

/// What to do about a refusal.
///
/// The Elixir is asymmetric here, and deliberately so rather than by accident: a
/// bad `client_id` or a `redirect_uri` that is not the application's own cannot
/// be answered with a redirect, because the redirect target is exactly what has
/// not been established as safe to send a browser to. Everything else is answered
/// by sending the browser back to the client, which is what an OAuth2 client is
/// listening for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Answer the browser here, and send it nowhere.
    Page,
    /// Send it back to the client with this error.
    Redirect {
        error: &'static str,
        description: &'static str,
    },
}

impl Refusal {
    /// A refusal the caller raises itself, all of which redirect: by the time
    /// these are reached the client and the redirect URI have both been checked.
    pub fn redirect(error: &'static str, description: &'static str) -> Self {
        Refusal::Redirect { error, description }
    }
}

/// What a refusal becomes: the page, or a trip back to the client carrying the
/// error and the state it sent.
///
/// The query is built with `Url` rather than with `format!` because the values
/// are the client's own — a `state` containing `&` or `=` would otherwise end the
/// parameter it is in, and the client would read someone else's query. The Elixir
/// does the same thing with `URI.encode_query/1`.
///
/// A `redirect_uri` that will not parse is answered with the page. That should
/// not happen, since `preauthorize` has already matched it against the
/// application's own registrations, but "should not happen" is not a reason to
/// send a browser somewhere unparsed.
pub fn answer(refusal: Refusal, redirect_uri: &str, state: Option<&str>) -> Response {
    let Refusal::Redirect { error, description } = refusal else {
        return page();
    };

    let mut pairs = vec![("error", error), ("error_description", description)];
    if let Some(state) = state {
        pairs.push(("state", state));
    }

    to_client(redirect_uri, &pairs)
}

/// Send the browser back to the client with these parameters, encoded.
///
/// The one place a redirect to a client is built, so that the error path and the
/// success path cannot disagree about how a redirect URI with a query of its own
/// is added to.
pub fn to_client(redirect_uri: &str, pairs: &[(&str, &str)]) -> Response {
    let Ok(mut url) = Url::parse(redirect_uri) else {
        return page();
    };

    {
        let mut query = url.query_pairs_mut();
        for (name, value) in pairs {
            query.append_pair(name, value);
        }
    }

    Redirect::to(url.as_str()).into_response()
}

/// The answer for anything that cannot be sent to a client.
fn page() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "invalid_request" })),
    )
        .into_response()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "invalid_token" })),
    )
        .into_response()
}

/// The answer to an approval that named something which is not a resource of
/// this service. It is RFC 8707's own error rather than the generic page, so
/// that the client is told which of the two mistakes it made.
fn invalid_target() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "invalid_target" })),
    )
        .into_response()
}

/// Only the one-time flow ID is submitted; permissions and targets stay server-side.
#[derive(Deserialize)]
pub struct ApproveForm {
    pub action: Option<String>,
    pub flow_id: Uuid,
}

/// The pairs of an `application/x-www-form-urlencoded` string, decoded.
///
/// `Url` does the decoding because it is already here and it does it the way the
/// specification says: `+` is a space, `%xx` is the byte, and a repeated name is
/// simply a second pair.
fn raw_query_pairs(raw: &str) -> Vec<(String, String)> {
    let Ok(mut url) = Url::parse("http://placeholder/") else {
        return Vec::new();
    };
    url.set_query(Some(raw));

    url.query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

/// Every value the pairs give one name, in the order they appear.
fn values_named(pairs: &[(String, String)], name: &str) -> Vec<String> {
    pairs
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .collect()
}

/// Approve exactly the request verified through Discord, once.
pub async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ApproveForm>,
) -> Result<Response, ApiError> {
    if form.action.as_deref() != Some("approve") {
        return Ok(page());
    }
    let Some(secret) = flow::binding(&headers, form.flow_id) else {
        return Ok(unauthorized());
    };
    if !super::csrf::same_origin(&headers, &state.links().site_url) {
        state
            .monitor()
            .observe(
                crate::security::Signal::CsrfRejected,
                "oauth2-approve",
                "the approval did not come from this site's own origin",
            )
            .await;
        return Err(ApiError::Forbidden("invalid_origin"));
    }
    let Some((pending, account_id)) = flow::load(state.pool(), form.flow_id, secret)
        .await
        .map_err(db)?
    else {
        return Ok(unauthorized());
    };
    let request = pending.request;
    let resources = match resources_of(&state, request.guild_id, pending.resource_uris).await {
        Ok(resources) => resources,
        Err(_) => return Ok(invalid_target()),
    };
    if describe(&state, &request, account_id).await.is_err() {
        return Ok(page());
    }

    let mut tx = state.pool().begin().await.map_err(db)?;
    if !flow::consume(&mut tx, form.flow_id, secret)
        .await
        .map_err(db)?
    {
        return Ok(unauthorized());
    }
    let code = match vc_core::application::authorize_in(
        &mut tx,
        request.guild_id,
        &request.scopes,
        &resources,
        &request.redirect_uri,
        &request.client_id,
        time::OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(code) => code,
        Err(_) => return Ok(page()),
    };
    tx.commit().await.map_err(db)?;
    let scope = request.scopes.join(" ");
    let guild_id = request.guild_id.to_string();
    let mut pairs = vec![
        ("code", code.as_str()),
        ("guild_id", guild_id.as_str()),
        ("scope", scope.as_str()),
    ];
    if let Some(state) = request.state.as_deref() {
        pairs.push(("state", state));
    }
    let mut response = private(to_client(&request.redirect_uri, &pairs));
    response.headers_mut().insert(
        SET_COOKIE,
        flow::clear_cookie(form.flow_id, state.secure_cookies()),
    );
    Ok(response)
}

fn db(error: sqlx::Error) -> ApiError {
    vc_core::Error::from(error).into()
}

fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
}

/// How `preauthorize`'s four answers are answered.
pub fn refusal_for(error: PreauthorizeError) -> Refusal {
    match error {
        // The two that must not redirect.
        PreauthorizeError::InvalidClientId | PreauthorizeError::InvalidRedirectUri => Refusal::Page,

        PreauthorizeError::InvalidScope => Refusal::redirect("invalid_request", "invalid_scope"),

        PreauthorizeError::InvalidApplicationGrantType => {
            Refusal::redirect("unauthorized_client", "invalid_application_grant_type")
        }
    }
}

/// The query an authorization request arrives as, with nothing required: the
/// Elixir answers a missing parameter with an OAuth error rather than a 400, so
/// absence has to reach the checks below instead of being rejected on the way in.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthorizeQuery {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub scope: Option<String>,
    pub guild_id: Option<String>,
    pub state: Option<String>,
}

/// A well-formed request: what the rest of the chain needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub guild_id: i64,
    pub state: Option<String>,
}

/// Why a request could not be started at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malformed {
    /// `invalid_request` with `invalid_response_type`. The Elixir renders an
    /// error for this rather than redirecting, because nothing has been checked
    /// yet — not even that the client exists.
    ResponseType,
    /// `invalid_client_id`, for its absence.
    NoClientId,
    /// `invalid_redirect_uri`, for its absence.
    NoRedirectUri,
    /// `invalid_request` with `invalid_guild_id`.
    NoGuildId,
}

impl Request {
    /// The request, or the reason it cannot be one.
    ///
    /// An omitted `scope` is refused, and that is the contract rather than an
    /// accident of the code that reaches it: `Authz.md` says the parameter's
    /// value "must be chosen" from the scope table, and RFC 6749 permits
    /// answering `invalid_scope` when there is no default to fall back on.
    ///
    /// The route there is roundabout. `Map.get(params, "scope", "")` gives `""`,
    /// and `String.split/2` returns `[""]` for it — "splitting on a non-existing
    /// pattern returns the original string", and empty parts are dropped only
    /// under `trim: true`, which the Elixir does not pass. So an omitted `scope`
    /// is *one empty scope* rather than none, and `is_valid_scopes?/1` refuses
    /// it. Writing `vec![]` here would quietly widen what the service accepts,
    /// which is the one thing the refusal's shape makes easy to do.
    pub fn parse(query: AuthorizeQuery) -> Result<Self, Malformed> {
        if query.response_type.as_deref() != Some("code") {
            return Err(Malformed::ResponseType);
        }

        let client_id = query.client_id.ok_or(Malformed::NoClientId)?;
        let redirect_uri = query.redirect_uri.ok_or(Malformed::NoRedirectUri)?;

        let guild_id = query
            .guild_id
            .and_then(|guild_id| guild_id.parse().ok())
            .ok_or(Malformed::NoGuildId)?;

        Ok(Request {
            client_id,
            redirect_uri,
            scopes: query
                .scope
                .unwrap_or_default()
                .split(' ')
                .map(str::to_owned)
                .collect(),
            guild_id,
            state: query.state,
        })
    }
}

/// What the screen shows, and what it has to post back to approve.
///
/// `resources` is the currencies the ask is narrowed to, as the units the screen
/// can name: empty is every currency of the guild, and one unit is a screen that
/// says so. It is what tells a person approving a grant for everything apart
/// from one approving a grant for one currency, which is the whole point of the
/// narrowing being visible rather than something only the API knows.
#[derive(Debug, Serialize)]
pub struct Consent {
    pub flow_id: Uuid,
    pub client_name: Option<String>,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub resources: Vec<String>,
    pub guild_id: i64,
    pub state: Option<String>,
}

/// Start a fresh Discord identity check for this exact authorization request.
pub async fn authorize(
    State(state): State<AppState>,
    Query(query): Query<AuthorizeQuery>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let request = match Request::parse(query) {
        Ok(request) => request,
        Err(_) => return Ok(page()),
    };
    if let Err(error) = vc_core::application::preauthorize(
        state.pool(),
        &request.scopes,
        &request.redirect_uri,
        &request.client_id,
    )
    .await
    {
        return Ok(answer(
            refusal_for(error),
            &request.redirect_uri,
            request.state.as_deref(),
        ));
    }
    let resource_uris = values_named(
        &raw_query_pairs(raw.as_deref().unwrap_or_default()),
        "resource",
    );
    if let Err(refusal) = resources_of(&state, request.guild_id, resource_uris.clone()).await {
        return Ok(answer(
            refusal,
            &request.redirect_uri,
            request.state.as_deref(),
        ));
    }
    let (id, secret) = flow::start(
        state.pool(),
        flow::Pending {
            request,
            resource_uris,
        },
    )
    .await
    .map_err(db)?;
    let mut response =
        private(Redirect::to(&state.discord().authorize_url(&id.to_string())).into_response());
    response
        .headers_mut()
        .insert(SET_COOKIE, flow::cookie(id, secret, state.secure_cookies()));
    Ok(response)
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub state: Uuid,
    pub code: Option<String>,
}

/// Verify Discord's response once, then show consent for the saved request.
/// No login cookie or API token is issued, and Discord credentials are not retained.
pub async fn discord_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, ApiError> {
    let Some(secret) = flow::binding(&headers, query.state) else {
        return Ok(unauthorized());
    };
    let Some(pending) = flow::claim(state.pool(), query.state, secret)
        .await
        .map_err(db)?
    else {
        return Ok(unauthorized());
    };
    let Some(code) = query.code else {
        return Ok(page());
    };
    let token = state.discord().exchange_code(&code).await?;
    let profile = state.discord().get_user_info(&token.token).await?;
    let Some(discord_id) = profile
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(crate::discord_id::parse)
    else {
        return Ok(unauthorized());
    };
    let account_id = vc_core::user::resolve_discord_id(state.pool(), discord_id)
        .await
        .map_err(db)?;
    let request = pending.request;
    let preauthorized = match vc_core::application::preauthorize(
        state.pool(),
        &request.scopes,
        &request.redirect_uri,
        &request.client_id,
    )
    .await
    {
        Ok(checked) => checked,
        Err(error) => {
            return Ok(answer(
                refusal_for(error),
                &request.redirect_uri,
                request.state.as_deref(),
            ));
        }
    };
    let resources = match resources_of(&state, request.guild_id, pending.resource_uris).await {
        Ok(resources) => resources,
        Err(refusal) => {
            return Ok(answer(
                refusal,
                &request.redirect_uri,
                request.state.as_deref(),
            ));
        }
    };
    if let Err(refusal) = describe(&state, &request, i64::from(account_id)).await {
        return Ok(answer(
            refusal,
            &request.redirect_uri,
            request.state.as_deref(),
        ));
    }
    let units = crate::resource::units(state.pool(), &resources)
        .await
        .map_err(db)?;
    if !flow::ready(state.pool(), query.state, i64::from(account_id))
        .await
        .map_err(db)?
    {
        return Ok(unauthorized());
    }
    Ok(private(consent::respond(
        &headers,
        Consent {
            flow_id: query.state,
            client_name: preauthorized.client_name,
            client_id: request.client_id,
            redirect_uri: request.redirect_uri,
            scopes: request.scopes,
            resources: units,
            guild_id: request.guild_id,
            state: request.state,
        },
    )))
}

/// Resolve an ask's `resource` values, as the refusal the consent screen answers
/// an invalid one with.
///
/// The browser flow's ask is always a guild's — the consent screen exists to
/// issue from a guild's pool — so the target the check is made against is the
/// guild the request named.
async fn resources_of(
    state: &AppState,
    guild_id: i64,
    values: Vec<String>,
) -> Result<Vec<i64>, Refusal> {
    crate::resource::resolve(
        state.pool(),
        &state.links().site_url,
        vc_core::grant::Target::Guild(guild_id),
        &values,
    )
    .await
    .map_err(|_| Refusal::redirect("invalid_target", "invalid_target"))
}

/// The guild checks, and the account's right to act for it.
///
/// Answers with the guild id, which the screen shows and which the code that
/// follows is bound to. The chain itself is [`crate::permissions::guild_access`],
/// because the page that allows a guild runs the same one — and the two answers it
/// can refuse with are what that returns `Unknown` and `Denied` for.
async fn describe(state: &AppState, request: &Request, account_id: i64) -> Result<i64, Refusal> {
    let Ok(account_id) = i32::try_from(account_id) else {
        return Err(Refusal::redirect("invalid_request", "permission_denied"));
    };

    match crate::permissions::guild_access(state, request.guild_id, account_id).await {
        crate::permissions::GuildAccess::Permitted => Ok(request.guild_id),
        crate::permissions::GuildAccess::Unknown => {
            Err(Refusal::redirect("invalid_request", "invalid_guild_id"))
        }
        crate::permissions::GuildAccess::Denied => {
            Err(Refusal::redirect("invalid_request", "permission_denied"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vc_core::application::check_scopes;

    /// The point of the distinction: a redirect_uri that is not the
    /// application's own is not a place to send a browser, so the refusal must
    /// stay here.
    #[test]
    fn an_untrusted_redirect_uri_is_not_redirected_to() {
        assert_eq!(
            refusal_for(PreauthorizeError::InvalidRedirectUri),
            Refusal::Page
        );
        assert_eq!(
            refusal_for(PreauthorizeError::InvalidClientId),
            Refusal::Page
        );
    }

    /// Once both of those have been checked, a refusal is safe to deliver to the
    /// client — which is the only way it learns what went wrong.
    #[test]
    fn the_rest_go_back_to_the_client() {
        assert_eq!(
            refusal_for(PreauthorizeError::InvalidScope),
            Refusal::Redirect {
                error: "invalid_request",
                description: "invalid_scope",
            }
        );
        assert_eq!(
            refusal_for(PreauthorizeError::InvalidApplicationGrantType),
            Refusal::Redirect {
                error: "unauthorized_client",
                description: "invalid_application_grant_type",
            }
        );
    }

    #[test]
    fn a_refusal_it_raises_itself_redirects() {
        assert_eq!(
            Refusal::redirect("invalid_request", "invalid_guild_id"),
            Refusal::Redirect {
                error: "invalid_request",
                description: "invalid_guild_id",
            }
        );
    }

    fn query(pairs: &[(&str, &str)]) -> AuthorizeQuery {
        let mut query = AuthorizeQuery::default();

        for (name, value) in pairs {
            match *name {
                "response_type" => query.response_type = Some((*value).to_owned()),
                "client_id" => query.client_id = Some((*value).to_owned()),
                "redirect_uri" => query.redirect_uri = Some((*value).to_owned()),
                "scope" => query.scope = Some((*value).to_owned()),
                "guild_id" => query.guild_id = Some((*value).to_owned()),
                "state" => query.state = Some((*value).to_owned()),
                other => panic!("no such parameter: {other}"),
            }
        }

        query
    }

    fn complete() -> Vec<(&'static str, &'static str)> {
        vec![
            ("response_type", "code"),
            ("client_id", "a-client"),
            ("redirect_uri", "https://app.example/callback"),
            ("scope", "vc.issue"),
            ("guild_id", "42"),
        ]
    }

    #[test]
    fn a_complete_request_is_one() {
        let parsed = Request::parse(query(&complete())).expect("a request");

        assert_eq!(parsed.client_id, "a-client");
        assert_eq!(parsed.redirect_uri, "https://app.example/callback");
        assert_eq!(parsed.scopes, ["vc.issue"]);
        assert_eq!(parsed.guild_id, 42);
        assert_eq!(parsed.state, None);
    }

    #[test]
    fn the_state_comes_back_out() {
        let mut pairs = complete();
        pairs.push(("state", "the-state"));

        let parsed = Request::parse(query(&pairs)).expect("a request");

        assert_eq!(parsed.state.as_deref(), Some("the-state"));
    }

    /// Anything but the code flow is refused before anything else is looked at.
    #[test]
    fn another_response_type_is_refused_first() {
        let mut pairs = complete();
        pairs[0] = ("response_type", "token");

        assert_eq!(Request::parse(query(&pairs)), Err(Malformed::ResponseType));
        assert_eq!(
            Request::parse(AuthorizeQuery::default()),
            Err(Malformed::ResponseType)
        );
    }

    #[test]
    fn the_required_parameters_are_required() {
        for (missing, expected) in [
            ("client_id", Malformed::NoClientId),
            ("redirect_uri", Malformed::NoRedirectUri),
            ("guild_id", Malformed::NoGuildId),
        ] {
            let pairs: Vec<_> = complete()
                .into_iter()
                .filter(|(name, _)| *name != missing)
                .collect();

            assert_eq!(Request::parse(query(&pairs)), Err(expected), "{missing}");
        }
    }

    /// A guild id that is not a number is a guild Discord does not have, which is
    /// the same answer the Elixir reaches by asking Discord about nonsense.
    #[test]
    fn a_guild_id_that_is_not_a_number_is_refused() {
        let mut pairs = complete();
        pairs[4] = ("guild_id", "not-a-number");

        assert_eq!(Request::parse(query(&pairs)), Err(Malformed::NoGuildId));
    }

    /// The surprising one, pinned so nobody "fixes" it into accepting more: an
    /// omitted scope is one empty scope, and the scope check refuses it.
    #[test]
    fn an_omitted_scope_is_one_empty_scope() {
        let pairs: Vec<_> = complete()
            .into_iter()
            .filter(|(name, _)| *name != "scope")
            .collect();

        let parsed = Request::parse(query(&pairs)).expect("a request");

        assert_eq!(parsed.scopes, [""]);
        assert!(check_scopes(&parsed.scopes).is_err());
    }

    #[test]
    fn several_scopes_arrive_split_on_spaces() {
        let mut pairs = complete();
        pairs[3] = ("scope", "vc.issue vc.pay");

        let parsed = Request::parse(query(&pairs)).expect("a request");

        assert_eq!(parsed.scopes, ["vc.issue", "vc.pay"]);
    }

    fn location(response: &Response) -> String {
        response
            .headers()
            .get(axum::http::header::LOCATION)
            .expect("a location")
            .to_str()
            .expect("a header")
            .to_owned()
    }

    fn redirecting() -> Refusal {
        Refusal::redirect("invalid_request", "invalid_guild_id")
    }

    #[test]
    fn a_redirecting_refusal_carries_the_error_and_the_state() {
        let response = answer(redirecting(), "https://app.example/callback", Some("abc"));

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            location(&response),
            "https://app.example/callback?error=invalid_request&error_description=invalid_guild_id&state=abc"
        );
    }

    /// The reason for `Url` rather than `format!`: the client's own values must
    /// not be able to end the parameter they are in.
    #[test]
    fn a_state_that_looks_like_a_query_is_escaped() {
        let response = answer(redirecting(), "https://app.example/callback", Some("a&b=c"));

        assert_eq!(
            location(&response),
            "https://app.example/callback?error=invalid_request&error_description=invalid_guild_id&state=a%26b%3Dc"
        );
    }

    #[test]
    fn a_redirect_uri_that_already_has_a_query_is_added_to() {
        let response = answer(redirecting(), "https://app.example/callback?x=1", None);

        assert_eq!(
            location(&response),
            "https://app.example/callback?x=1&error=invalid_request&error_description=invalid_guild_id"
        );
    }

    #[test]
    fn a_page_refusal_goes_nowhere() {
        let response = answer(Refusal::Page, "https://app.example/callback", Some("abc"));

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .is_none()
        );
    }

    /// Belt and braces: the URI has already been checked, and it is still not
    /// worth sending a browser to something that will not parse.
    #[test]
    fn an_unparseable_redirect_uri_is_answered_here_instead() {
        let response = answer(redirecting(), "not a url", None);

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .is_none()
        );
    }
}
