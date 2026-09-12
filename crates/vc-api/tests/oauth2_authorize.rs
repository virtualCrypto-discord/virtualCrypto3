//! The consent screen's first answers.
//!
//! There is no Elixir test for any of this — OAuth2 is the one area of the
//! migration without a ported spec — so these are additions.

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::http::header::LOCATION;
use sqlx::PgPool;
use support::{fake, state};
use tower::ServiceExt;

async fn visit(app: Router, uri: &str) -> (u16, String) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    let location = response
        .headers()
        .get(LOCATION)
        .map(|value| value.to_str().expect("a header").to_owned())
        .unwrap_or_default();

    (response.status().as_u16(), location)
}

const A_REQUEST: &str = "/oauth2/authorize\
    ?response_type=code\
    &client_id=a-client\
    &redirect_uri=https%3A%2F%2Fapp.example%2Fcallback\
    &scope=openid\
    &guild_id=1";

/// A browser is a person: where an SPA's own request would be answered `401`,
/// a navigation goes to the login page and comes back.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_browser_without_a_session_is_sent_to_log_in(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, location) = visit(app, A_REQUEST).await;

    assert_eq!(status, 303);
    assert!(location.starts_with("/login?continue="), "{location}");

    // Back to here rather than to the front page, and encoded so that the second
    // request's own query survives the trip.
    assert!(location.contains("oauth2%2Fauthorize"), "{location}");
    assert!(location.contains("%3Fresponse_type%3Dcode"), "{location}");
}

/// Nothing is looked up until the request is one: the response type is checked
/// before the session, the client and the guild.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_request_that_is_not_the_code_flow_is_answered_here(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, location) = visit(app, "/oauth2/authorize?response_type=token&client_id=x").await;

    assert_eq!(status, 400);
    assert_eq!(location, "", "and it goes nowhere");
}

async fn post(app: Router, body: &str) -> (u16, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth2/authorize")
                .header(
                    axum::http::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(Body::from(body.to_owned()))
                .expect("request"),
        )
        .await
        .expect("router response");

    let location = response
        .headers()
        .get(LOCATION)
        .map(|value| value.to_str().expect("a header").to_owned())
        .unwrap_or_default();

    (response.status().as_u16(), location)
}

const AN_APPROVAL: &str = "action=approve\
    &response_type=code\
    &client_id=a-client\
    &redirect_uri=https%3A%2F%2Fapp.example%2Fcallback\
    &scope=openid\
    &guild_id=1";

/// The Elixir has no clause for anything but `approve`, so a request without it
/// crashes rather than being decided. A refusal is the decision that was missing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_action_that_is_not_approval_is_answered_here(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, location) = post(app, &AN_APPROVAL.replace("approve", "deny")).await;

    assert_eq!(status, 400);
    assert_eq!(location, "", "and it goes nowhere");
}

/// An approval from nobody is a 401 rather than a trip to the login page: a POST
/// cannot be resumed by a navigation, so the SPA has to decide what to do.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_post_without_a_session_is_not_approved(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, location) = post(app, AN_APPROVAL).await;

    assert_eq!(status, 401);
    assert_eq!(location, "", "and it goes nowhere");
}

/// However far a POST gets, it never redirects to the client — which is the
/// difference between this path and the `GET`, and the reason it has its own
/// tests rather than borrowing them.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_post_never_redirects_to_the_client(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, location) = post(app, &AN_APPROVAL.replace("code", "token")).await;

    assert_eq!(status, 400);
    assert_eq!(location, "", "and it goes nowhere");
}
