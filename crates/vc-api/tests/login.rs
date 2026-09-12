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
use axum::http::header::{LOCATION, SET_COOKIE};
use sqlx::PgPool;
use support::{SESSION_SECRET, fake, state};
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
