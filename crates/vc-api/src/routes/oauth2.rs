//! The consent screen.
//!
//! What is here so far is the part of it worth getting wrong-proof separately:
//! how a refusal is answered. The rest of the screen is a validation chain, and
//! each link of it lands on one of these.

use axum::Json;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::session;
use crate::state::AppState;
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

/// The body the consent form posts back.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthorizeForm {
    pub response_type: Option<String>,
    pub action: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub scope: Option<String>,
    pub guild_id: Option<String>,
    pub state: Option<String>,
}

impl From<AuthorizeForm> for AuthorizeQuery {
    fn from(form: AuthorizeForm) -> Self {
        Self {
            response_type: form.response_type,
            client_id: form.client_id,
            redirect_uri: form.redirect_uri,
            scope: form.scope,
            guild_id: form.guild_id,
            state: form.state,
        }
    }
}

/// `POST /oauth2/authorize`: the same checks, and then a code.
///
/// Unlike the `GET`, **nothing here is answered with a redirect to the client**,
/// which is not an oversight: an approval that fails has not established that
/// anything is approved, and the Elixir renders its error page for all of them.
pub async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AuthorizeForm>,
) -> Response {
    // The only action there is. There is no deny: the Elixir has no clause for
    // one, so its answer to a request without `approve` is a crash rather than
    // a decision, and a refusal is the decision that was missing.
    if form.action.as_deref() != Some("approve") {
        return page();
    }

    let Ok(request) = Request::parse(form.into()) else {
        return page();
    };

    let Some(session) = session::from_headers(&headers, state.session_secret()) else {
        return unauthorized();
    };
    let Some(account_id) = session.user_id else {
        return unauthorized();
    };

    if vc_core::application::preauthorize(
        state.pool(),
        &request.scopes,
        &request.redirect_uri,
        &request.client_id,
    )
    .await
    .is_err()
    {
        return page();
    }

    if describe(&state, &request, account_id).await.is_err() {
        return page();
    }

    let issued = vc_core::application::authorize(
        state.pool(),
        request.guild_id,
        &request.scopes,
        &request.redirect_uri,
        &request.client_id,
        time::OffsetDateTime::now_utc(),
    )
    .await;

    let Ok(code) = issued else {
        return page();
    };

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

    to_client(&request.redirect_uri, &pairs)
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl Malformed {
    pub fn refusal(self) -> Refusal {
        match self {
            Malformed::ResponseType | Malformed::NoClientId | Malformed::NoRedirectUri => {
                Refusal::Page
            }
            Malformed::NoGuildId => Refusal::redirect("invalid_request", "invalid_guild_id"),
        }
    }
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
#[derive(Debug, Serialize)]
pub struct Consent {
    pub client_name: Option<String>,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub guild_id: i64,
    pub state: Option<String>,
}

/// `GET /oauth2/authorize`: check the request, then describe the consent.
///
/// The browser is sent to `/login` rather than answered when it has no session.
/// An SPA's own requests want a 401, but this is a navigation, and a person who
/// is not logged in is a person who should log in and come back.
pub async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(query): Query<AuthorizeQuery>,
) -> Response {
    // Kept before the parse takes the query, because a malformed request still
    // has somewhere to be sent if it named a redirect URI.
    let redirect_uri = query.redirect_uri.clone();
    let sent_state = query.state.clone();

    let request = match Request::parse(query) {
        Ok(request) => request,
        Err(malformed) => {
            let refusal = malformed.refusal();

            return match (refusal, redirect_uri.as_deref()) {
                (Refusal::Redirect { .. }, Some(redirect_uri)) => {
                    answer(refusal, redirect_uri, sent_state.as_deref())
                }
                _ => answer(Refusal::Page, redirect_uri.as_deref().unwrap_or(""), None),
            };
        }
    };

    let Some(session) = session::from_headers(&headers, state.session_secret()) else {
        return to_login(&uri);
    };
    let Some(account_id) = session.user_id else {
        return to_login(&uri);
    };

    let preauthorized = match vc_core::application::preauthorize(
        state.pool(),
        &request.scopes,
        &request.redirect_uri,
        &request.client_id,
    )
    .await
    {
        Ok(preauthorized) => preauthorized,
        Err(error) => {
            return answer(
                refusal_for(error),
                &request.redirect_uri,
                request.state.as_deref(),
            );
        }
    };

    match describe(&state, &request, account_id).await {
        Ok(guild_id) => Json(Consent {
            client_name: preauthorized.client_name,
            client_id: request.client_id,
            redirect_uri: request.redirect_uri,
            scopes: request.scopes,
            guild_id,
            state: request.state,
        })
        .into_response(),
        Err(refusal) => answer(refusal, &request.redirect_uri, request.state.as_deref()),
    }
}

/// Send the browser to log in and come back here.
fn to_login(uri: &Uri) -> Response {
    let here = uri
        .path_and_query()
        .map(axum::http::uri::PathAndQuery::as_str)
        .unwrap_or("/");

    // Encoded rather than interpolated, for the same reason a refusal is: the
    // value is partly the browser's own.
    let mut login = Url::parse("http://placeholder/").expect("a base url");
    login.set_path("/login");
    login.query_pairs_mut().append_pair("continue", here);

    // `Url` has no `path_and_query`, and does not need one: the pair is the path
    // and whatever the encoder produced as the query.
    let login = format!(
        "{}{}",
        login.path(),
        login
            .query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default()
    );

    Redirect::to(&login).into_response()
}

/// The guild checks, and the account's right to act for it.
///
/// Answers with the guild id, which the screen shows and which the code that
/// follows is bound to.
async fn describe(state: &AppState, request: &Request, account_id: i64) -> Result<i64, Refusal> {
    let guild = state
        .discord()
        .get_guild(request.guild_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| Refusal::redirect("invalid_request", "invalid_guild_id"))?;

    // A grant belongs to a guild, so the bot has to be in it: one the bot cannot
    // see is not a guild it can grant anything in.
    let in_guild = state
        .discord()
        .get_guild_member(request.guild_id, state.discord().bot_user_id())
        .await
        .ok()
        .flatten()
        .is_some();

    if !in_guild {
        return Err(Refusal::redirect("invalid_request", "invalid_guild_id"));
    }

    let account = vc_core::user::find_by_id(state.pool(), account_id as i32)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| Refusal::redirect("invalid_request", "permission_denied"))?;

    // The Elixir used the session's VirtualCrypto id as a Discord id here, which
    // is why its answer was always no.
    let Some(discord_id) = account.discord_id else {
        return Err(Refusal::redirect("invalid_request", "permission_denied"));
    };

    let member = state
        .discord()
        .get_guild_member(request.guild_id, discord_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| Refusal::redirect("invalid_request", "invalid_guild_id"))?;

    let roles = state
        .discord()
        .get_roles(request.guild_id)
        .await
        .unwrap_or_default();

    let facts = crate::permissions::guild_facts(&guild, &member, &roles)
        .ok_or_else(|| Refusal::redirect("invalid_request", "invalid_guild_id"))?;

    if !crate::permissions::may_act_for_guild(discord_id, &facts) {
        return Err(Refusal::redirect("invalid_request", "permission_denied"));
    }

    Ok(request.guild_id)
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
            ("scope", "openid"),
            ("guild_id", "42"),
        ]
    }

    #[test]
    fn a_complete_request_is_one() {
        let parsed = Request::parse(query(&complete())).expect("a request");

        assert_eq!(parsed.client_id, "a-client");
        assert_eq!(parsed.redirect_uri, "https://app.example/callback");
        assert_eq!(parsed.scopes, ["openid"]);
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
        pairs[3] = ("scope", "openid profile");

        let parsed = Request::parse(query(&pairs)).expect("a request");

        assert_eq!(parsed.scopes, ["openid", "profile"]);
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
