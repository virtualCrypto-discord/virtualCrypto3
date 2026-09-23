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
    &scope=vc.issue\
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
    &scope=vc.issue\
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

/// Invalid approvals never redirect to the client.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_invalid_post_never_redirects_to_the_client(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));

    let (status, location) = post(app, &AN_APPROVAL.replace("code", "token")).await;

    assert_eq!(status, 400);
    assert_eq!(location, "", "and it goes nowhere");
}

/// Submit the actual hidden inputs emitted by the consent page, decoding HTML
/// entities as a browser does before form encoding. This catches missing fields
/// and resource values accidentally replaced with their display units.
fn hidden_inputs(html: &str) -> Vec<(String, String)> {
    html.split("<input ")
        .skip(1)
        .map(|input| {
            let tag = input.split_once('>').expect("closed input").0;
            let attribute = |name| {
                tag.split_once(&format!("{name}=\""))
                    .expect("attribute")
                    .1
                    .split_once('"')
                    .expect("closed attribute")
                    .0
                    .replace("&quot;", "\"")
                    .replace("&#39;", "'")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&")
            };
            assert_eq!(attribute("type"), "hidden");
            (attribute("name"), attribute("value"))
        })
        .collect()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn browser_consent_preserves_the_request_through_approval_and_token_exchange(pool: PgPool) {
    use support::{MONEY_GUILD, MONEY_USER1};
    let money = support::setup_money(&pool).await;
    let name = "<script>alert('app')</script> & App";
    let application = support::insert_application(&pool, MONEY_USER1, name).await;
    sqlx::query("UPDATE applications SET grant_types = ARRAY['authorization_code']::openid_connect_grant_types[] WHERE id=$1")
        .bind(application).execute(&pool).await.unwrap();
    let redirect_uri = "https://app.example/callback?x=one&y=two";
    sqlx::query("INSERT INTO redirect_uris(application_id,redirect_uri,inserted_at,updated_at) VALUES($1,$2,now(),now())")
        .bind(application).bind(redirect_uri).execute(&pool).await.unwrap();
    let client = support::client_id_of(&pool, application).await;
    let app = vc_api::router(state(
        pool.clone(),
        support::FakeDiscord::with_member(MONEY_USER1, &[], &[]),
    ));
    let session = vc_api::session::Session::logged_in(1)
        .sign(support::SESSION_SECRET.as_bytes())
        .unwrap();
    let cookie = format!("{}={session}", vc_api::session::COOKIE_NAME);
    let client_state = "\"><script>alert('state')</script>&literal=&quot;";
    let collection = format!("{}/api/v2/currencies", support::links().site_url);
    let currency = format!("{collection}/{}", money.currency);

    // No indicator, the collection indicator, and repeated currency indicators
    // must all survive the browser round trip without widening the grant.
    for resources in [vec![], vec![collection], vec![currency.clone(), currency]] {
        let expected = if resources
            .first()
            .is_some_and(|uri| uri.ends_with(&format!("/{}", money.currency)))
        {
            vec![money.currency]
        } else {
            vec![]
        };
        let mut url = reqwest::Url::parse("https://vc.example/oauth2/authorize").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", &client),
            ("redirect_uri", redirect_uri),
            ("scope", "vc.issue"),
            ("guild_id", &MONEY_GUILD.to_string()),
            ("state", client_state),
        ]);
        for resource in &resources {
            url.query_pairs_mut().append_pair("resource", resource);
        }
        let uri = format!("{}?{}", url.path(), url.query().unwrap());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header("cookie", &cookie)
                    .header("accept", "text/html,application/xhtml+xml;q=0.9")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<form method=\"post\" action=\"/oauth2/authorize\">"));
        assert!(html.contains("<button type=\"submit\">"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains(if expected.is_empty() {
            "このサーバーのすべての通貨"
        } else {
            "<li>n</li>"
        }));
        let inputs = hidden_inputs(&html);
        assert_eq!(
            inputs
                .iter()
                .filter(|(name, _)| name == "resource")
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>(),
            resources
        );
        let mut form = reqwest::Url::parse("https://vc.example/").unwrap();
        form.query_pairs_mut().extend_pairs(&inputs);
        let approved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth2/authorize")
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form.query().unwrap().to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approved.status(), 303);
        let destination =
            reqwest::Url::parse(approved.headers()[LOCATION].to_str().unwrap()).unwrap();
        let pairs: std::collections::HashMap<_, _> =
            destination.query_pairs().into_owned().collect();
        assert_eq!(pairs["state"], client_state);
        assert_eq!(pairs["guild_id"], MONEY_GUILD.to_string());
        assert_eq!(pairs["scope"], "vc.issue");
        assert_eq!(pairs["y"], "two");

        let exchange = serde_json::json!({"grant_type": "authorization_code", "client_id": client, "redirect_uri": redirect_uri, "code": pairs["code"]});
        let exchanged = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth2/token")
                    .header("content-type", "application/json")
                    .body(Body::from(exchange.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(exchanged.status(), 200);
        let bytes = axum::body::to_bytes(exchanged.into_body(), usize::MAX)
            .await
            .unwrap();
        let token: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(token["access_token"].as_str().is_some());
        let stored: Vec<i64> = sqlx::query_scalar(
            "SELECT r.currency_id FROM grant_resources r JOIN access_tokens t ON t.grant_id=r.grant_id WHERE t.token_id=$1",
        )
        .bind(uuid::Uuid::parse_str(token["access_token"].as_str().unwrap()).unwrap())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(stored, expected);

        for accept in ["application/json", "text/html;q=0,application/json"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(&uri)
                        .header("cookie", &cookie)
                        .header("accept", accept)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-type"], "application/json");
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let consent: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(consent["client_name"], name);
            assert_eq!(consent["state"], client_state);
            assert_eq!(
                consent["resources"],
                if expected.is_empty() {
                    serde_json::json!([])
                } else {
                    serde_json::json!(["n"])
                }
            );
        }
    }
}

/// An invalid guild must not turn the consent endpoint into an open redirect.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn malformed_requests_never_redirect_to_an_unverified_destination(pool: PgPool) {
    for suffix in [
        "",
        "&guild_id=",
        "&guild_id=invalid",
        "&guild_id=9223372036854775808",
    ] {
        let uri = format!(
            "/oauth2/authorize?response_type=code&client_id=unregistered&redirect_uri=https%3A%2F%2Funregistered.example%2Fcallback&scope=vc.issue&state=client-state{suffix}"
        );
        let (status, location) = visit(vc_api::router(state(pool.clone(), fake())), &uri).await;
        assert_eq!(status, 400, "{suffix}: {location}");
        assert!(
            location.is_empty(),
            "an unverified destination must never receive a redirect: {location}"
        );
    }
}
