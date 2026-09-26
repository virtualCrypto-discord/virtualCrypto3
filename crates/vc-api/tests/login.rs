//! The two pages a browser visits: sending it to Discord with a state, and
//! ending the session afterwards.
//!
//! The redirect is 303, which is axum's `Redirect::to`. Phoenix's `redirect/2`
//! sends 302, and for a browser navigating a GET the two are the same thing —
//! but it is a difference, and it is recorded in docs/known-gaps.md rather than
//! left for someone to notice.

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use sqlx::PgPool;
use support::{JWT_SECRET, SESSION_SECRET, fake, links, state};
use tower::ServiceExt;
use vc_api::session::{COOKIE_NAME, Session};

async fn visit(app: Router, uri: &str) -> (u16, String, String) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    let header = |name| {
        response
            .headers()
            .get(name)
            .map(|value| value.to_str().expect("a header").to_owned())
            .unwrap_or_default()
    };

    (
        response.status().as_u16(),
        header(LOCATION),
        header(SET_COOKIE),
    )
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn logging_in_sends_the_browser_to_discord(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, location, _) = visit(app, "/login").await;

    assert_eq!(status, 303);
    assert!(
        location.starts_with("https://discord.test/authorize?state="),
        "{location}"
    );
}

/// The state is the whole point of the redirect: the callback can only refuse an
/// answer it did not ask for if the session remembers what it asked for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn logging_in_remembers_the_state_it_sent(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (_, location, cookie) = visit(app, "/login?continue=/me").await;

    let sent = location
        .split("state=")
        .nth(1)
        .expect("a state in the redirect");

    let value = cookie
        .split(';')
        .next()
        .expect("the cookie itself")
        .strip_prefix(&format!("{COOKIE_NAME}="))
        .expect("the session cookie");

    let session = Session::parse(value, SESSION_SECRET.as_bytes()).expect("a session");
    let attempt = session.discord_oauth2.expect("a login in flight");

    assert_eq!(attempt.state, sent);
    assert_eq!(attempt.continue_to, "/me");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn logging_in_without_saying_where_to_return_goes_home(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (_, _, cookie) = visit(app, "/login").await;
    let value = cookie
        .split(';')
        .next()
        .expect("the cookie itself")
        .strip_prefix(&format!("{COOKIE_NAME}="))
        .expect("the session cookie");

    let session = Session::parse(value, SESSION_SECRET.as_bytes()).expect("a session");

    assert_eq!(
        session
            .discord_oauth2
            .expect("a login in flight")
            .continue_to,
        "/"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn logging_out_expires_the_cookie(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, to, cookie) = visit(app, "/logout").await;

    assert_eq!(status, 303);
    assert_eq!(to, "/");
    assert!(cookie.contains("Max-Age=0"), "{cookie}");
}

/// The login in full: the state `/login` sent, then Discord's answer to it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_callback_logs_the_account_in(pool: PgPool) {
    let app = vc_api::router(state(pool.clone(), fake()));

    let (_, location, cookie) = visit(app.clone(), "/login?continue=/me").await;
    let sent = location
        .split("state=")
        .nth(1)
        .expect("a state in the redirect")
        .to_owned();

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/callback/discord?state={sent}&code=the-code"))
                .header(COOKIE, cookie.split(';').next().expect("the cookie"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 303);
    assert_eq!(
        response
            .headers()
            .get(LOCATION)
            .expect("a location")
            .to_str()
            .expect("a header"),
        "/me"
    );

    let value = response
        .headers()
        .get(SET_COOKIE)
        .expect("a cookie")
        .to_str()
        .expect("a header")
        .split(';')
        .next()
        .expect("the cookie itself")
        .strip_prefix(&format!("{COOKIE_NAME}="))
        .expect("the session cookie")
        .to_owned();

    let session = Session::parse(&value, SESSION_SECRET.as_bytes()).expect("a session");

    assert!(session.user_id.is_some(), "somebody is logged in");
    assert!(
        session.discord_oauth2.is_none(),
        "the login is no longer in flight"
    );

    let stored = sqlx::query!("SELECT COUNT(*) AS count FROM discord_users")
        .fetch_one(&pool)
        .await
        .expect("count the authorizations");

    assert_eq!(stored.count, Some(1), "the authorization was recorded");
}

/// An answer to a login this browser never started is not an answer.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_callback_whose_state_does_not_match_is_refused(pool: PgPool) {
    let app = vc_api::router(state(pool.clone(), fake()));

    let (_, _, cookie) = visit(app.clone(), "/login").await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/callback/discord?state=not-the-one-we-sent&code=the-code")
                .header(COOKIE, cookie.split(';').next().expect("the cookie"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 303);
    assert_eq!(
        response
            .headers()
            .get(LOCATION)
            .expect("a location")
            .to_str()
            .expect("a header"),
        "/"
    );

    let stored = sqlx::query!("SELECT COUNT(*) AS count FROM discord_users")
        .fetch_one(&pool)
        .await
        .expect("count the authorizations");

    assert_eq!(stored.count, Some(0), "nothing was recorded");
}

/// Walk a browser through the whole login and hand back the cookie it ends with.
async fn logged_in(app: Router, pool: &PgPool) -> String {
    let _ = pool;

    let (_, location, cookie) = visit(app.clone(), "/login?continue=/me").await;
    let sent = location
        .split("state=")
        .nth(1)
        .expect("a state in the redirect")
        .to_owned();

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/callback/discord?state={sent}&code=the-code"))
                .header(COOKIE, cookie.split(';').next().expect("the cookie"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    response
        .headers()
        .get(SET_COOKIE)
        .expect("a cookie")
        .to_str()
        .expect("a header")
        .split(';')
        .next()
        .expect("the cookie itself")
        .to_owned()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_session_can_get_a_token(pool: PgPool) {
    let app = vc_api::router(state(pool.clone(), fake()));
    let cookie = logged_in(app.clone(), &pool).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/token")
                .header(COOKIE, cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let token: serde_json::Value = serde_json::from_slice(&body).expect("json");

    assert_eq!(token["expires_in"], 3600);

    let claims = vc_auth::jwt::verify(
        token["access_token"].as_str().expect("a token"),
        JWT_SECRET.as_bytes(),
    )
    .expect("a token the API would accept");

    assert_eq!(claims.kind, "user");
    assert_eq!(claims.scopes, ["oauth2.register", "vc.pay", "vc.claim"]);

    // The row is the point of issuing it here rather than by hand: a token whose
    // id is not recorded cannot be revoked.
    let recorded = sqlx::query!("SELECT COUNT(*) AS count FROM user_access_tokens")
        .fetch_one(&pool)
        .await
        .expect("count the tokens");

    assert_eq!(recorded.count, Some(1));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn without_a_session_there_is_no_token(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 401);
}

/// `/invite` and `/support`: the two addresses the old site kept for a browser that
/// wanted the bot or the guild, answered from the same settings the command responses
/// read, and as redirects because there is nothing of ours to show.
///
/// Asserted against `support::links()`, which is what the state under test carries, so
/// this says the setting is used rather than that a URL looks plausible.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_bot_and_guild_addresses_are_redirects(pool: PgPool) {
    let configured = links();
    let app = vc_api::router(state(pool, fake()));

    let (status, location, _) = visit(app.clone(), "/invite").await;

    assert_eq!(status, 307);
    assert_eq!(location, configured.invite_url);

    let (status, location, _) = visit(app, "/support").await;

    assert_eq!(status, 307);
    assert_eq!(location, configured.support_guild_invite_url);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn login_cannot_redirect_to_an_external_continue(pool: PgPool) {
    for target in [
        "https%3A%2F%2Funtrusted.example%2Flogin",
        "%2F%2Funtrusted.example",
        "%2F%5Cuntrusted.example",
        "%2F%09%2Funtrusted.example",
        "javascript%3Aalert(1)",
    ] {
        let app = vc_api::router(state(pool.clone(), fake()));

        let (_, location, cookie) = visit(app.clone(), &format!("/login?continue={target}")).await;
        let sent = location
            .split("state=")
            .nth(1)
            .expect("a state in the redirect")
            .to_owned();

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/callback/discord?state={sent}&code=the-code"))
                    .header(COOKIE, cookie.split(';').next().expect("the cookie"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status().as_u16(), 303);
        assert_eq!(
            response
                .headers()
                .get(LOCATION)
                .expect("a location")
                .to_str()
                .expect("a header"),
            "/"
        );

        let value = response
            .headers()
            .get(SET_COOKIE)
            .expect("a cookie")
            .to_str()
            .expect("a header")
            .split(';')
            .next()
            .expect("the cookie itself")
            .strip_prefix(&format!("{COOKIE_NAME}="))
            .expect("the session cookie")
            .to_owned();

        let session = Session::parse(&value, SESSION_SECRET.as_bytes()).expect("a session");

        assert!(session.user_id.is_some(), "somebody is logged in");
        assert!(
            session.discord_oauth2.is_none(),
            "the login is no longer in flight"
        );

        let stored = sqlx::query!("SELECT COUNT(*) AS count FROM discord_users")
            .fetch_one(&pool)
            .await
            .expect("count the authorizations");

        assert_eq!(stored.count, Some(1), "the authorization was recorded");
    }
}

#[test]
fn real_discord_authorization_url_preserves_the_endpoint_and_parameters() {
    use vc_api::discord::{DiscordApi, HttpDiscordApi};
    let redirect = "https://vc.example/callback/discord?first=1&second=2";
    let state = "state+with&reserved=characters";
    let discord = HttpDiscordApi::new("123", "unused", "unused", redirect);
    let address = reqwest::Url::parse(&discord.authorize_url(state)).unwrap();
    assert_eq!(
        address.origin().ascii_serialization(),
        "https://discord.com"
    );
    assert_eq!(address.path(), "/api/oauth2/authorize");
    let parameters: std::collections::HashMap<_, _> = address.query_pairs().collect();
    assert_eq!(parameters.len(), 6);
    assert_eq!(parameters["client_id"], "123");
    assert_eq!(parameters["redirect_uri"], redirect);
    assert_eq!(parameters["state"], state);
    assert_eq!(parameters["response_type"], "code");
    assert_eq!(parameters["scope"], "identify");
    assert_eq!(parameters["prompt"], "none");
}

/// A copied cookie must not renew access after logout, while another login stays valid.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn logout_revokes_the_cookie_on_every_authenticated_browser_route(pool: PgPool) {
    let app = vc_api::router(state(pool.clone(), fake()));
    let first = logged_in(app.clone(), &pool).await;
    let second = logged_in(app.clone(), &pool).await;
    assert_ne!(first, second);
    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/logout")
                .header(COOKIE, &first)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status().as_u16(), 303);
    assert!(
        logout.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );

    for (cookie, status) in [(&first, 401), (&second, 200)] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
    }

    let query = "response_type=code&client_id=00000000-0000-4000-8000-000000000001&redirect_uri=https%3A%2F%2Fapp.example%2Fcallback&scope=vc.issue&guild_id=1";
    let consent = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/oauth2/authorize?{query}"))
                .header(COOKIE, &first)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(consent.status().as_u16(), 303);
    assert!(
        consent.headers()[LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/login")
    );
    let approval = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth2/authorize")
                .header(COOKIE, &first)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("origin", links().site_url)
                .body(Body::from(format!("action=approve&{query}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approval.status().as_u16(), 401);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn expired_or_unregistered_sessions_cannot_issue_tokens(pool: PgPool) {
    let app = vc_api::router(state(pool.clone(), fake()));
    let cookie = logged_in(app.clone(), &pool).await;
    sqlx::query("UPDATE browser_sessions SET expires = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let unregistered = format!(
        "{COOKIE_NAME}={}",
        Session::logged_in(1)
            .sign(SESSION_SECRET.as_bytes())
            .unwrap()
    );
    for cookie in [cookie, unregistered] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 401);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM user_access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
