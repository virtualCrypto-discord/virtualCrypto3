//! A new Discord identity check for every request, with one browser-bound approval.
mod support;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, header::LOCATION},
    response::Response,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::*;
use tower::ServiceExt;

const REDIRECT: &str = "https://app.example/callback?x=one&y=two";
const CLIENT_STATE: &str = "\"><script>alert('state')</script>&literal=&quot;";
const NAME: &str = "<script>alert('app')</script> & App";

struct Fixture {
    app: Router,
    client: String,
    application: i64,
}
async fn fixture(pool: &PgPool) -> Fixture {
    setup_money(pool).await;
    let application = insert_application(pool, MONEY_USER1, NAME).await;
    sqlx::query("UPDATE applications SET grant_types=ARRAY['authorization_code']::openid_connect_grant_types[] WHERE id=$1")
        .bind(application).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO redirect_uris(application_id,redirect_uri,inserted_at,updated_at) VALUES($1,$2,now(),now())")
        .bind(application).bind(REDIRECT).execute(pool).await.unwrap();
    Fixture {
        app: vc_api::router(state(
            pool.clone(),
            FakeDiscord::with_member(MONEY_USER1, &[], &[]),
        )),
        client: client_id_of(pool, application).await,
        application,
    }
}
fn uri(client: &str, resources: &[String]) -> String {
    let mut url = reqwest::Url::parse("https://vc.example/oauth2/authorize").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", client),
        ("redirect_uri", REDIRECT),
        ("scope", "vc.issue"),
        ("guild_id", &MONEY_GUILD.to_string()),
        ("state", CLIENT_STATE),
    ]);
    for resource in resources {
        url.query_pairs_mut().append_pair("resource", resource);
    }
    format!("{}?{}", url.path(), url.query().unwrap())
}
async fn get(app: &Router, uri: &str, cookie: Option<&str>, accept: &str) -> Response {
    let mut request = Request::builder().uri(uri).header("accept", accept);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}
struct Flow {
    id: String,
    cookie: String,
}
async fn start(f: &Fixture, resources: &[String]) -> Flow {
    let response = get(&f.app, &uri(&f.client, resources), None, "text/html").await;
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let url = reqwest::Url::parse(response.headers()[LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("discord.test"));
    let id = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    assert_ne!(id, CLIENT_STATE);
    let set_cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));
    assert!(set_cookie.contains("Max-Age=600"));
    let cookie = set_cookie.split(';').next().unwrap().to_owned();
    assert!(!cookie.starts_with("_virtualcrypto_session="));
    Flow { id, cookie }
}
async fn callback(app: &Router, flow: &Flow, cookie: Option<&str>, accept: &str) -> Response {
    get(
        app,
        &format!("/callback/discord?state={}&code=the-code", flow.id),
        cookie,
        accept,
    )
    .await
}
async fn post(app: &Router, flow: &Flow, body: &str, fields: &[(&str, &str)]) -> Response {
    let mut request = Request::builder()
        .method("POST")
        .uri("/oauth2/authorize")
        .header("cookie", &flow.cookie)
        .header("content-type", "application/x-www-form-urlencoded");
    for (name, value) in fields {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}
fn form(flow: &Flow) -> String {
    format!("action=approve&flow_id={}", flow.id)
}
async fn approve(app: &Router, flow: &Flow) -> Response {
    post(app, flow, &form(flow), &[("sec-fetch-site", "same-origin")]).await
}
async fn count_codes(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM authorization_codes")
        .fetch_one(pool)
        .await
        .unwrap()
}
fn hidden_inputs(html: &str) -> Vec<(String, String)> {
    html.split("<input ")
        .skip(1)
        .map(|input| {
            let tag = input.split_once('>').unwrap().0;
            let attribute = |name| {
                tag.split_once(&format!("{name}=\""))
                    .unwrap()
                    .1
                    .split_once('"')
                    .unwrap()
                    .0
                    .to_owned()
            };
            (attribute("name"), attribute("value"))
        })
        .collect()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn browser_request_survives_discord_consent_and_token_exchange(pool: PgPool) {
    let f = fixture(&pool).await;
    let collection = format!("{}/api/v2/currencies", links().site_url);
    let currency = format!("{collection}/1");
    for resources in [vec![], vec![collection], vec![currency.clone(), currency]] {
        let restricted = resources.first().is_some_and(|r| r.ends_with("/1"));
        let flow = start(&f, &resources).await;
        let response = callback(
            &f.app,
            &flow,
            Some(&flow.cookie),
            "text/html,application/xhtml+xml;q=0.9",
        )
        .await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        assert!(!response.headers().contains_key("set-cookie"));
        let html = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains(if restricted {
            "<li>n</li>"
        } else {
            "このサーバーのすべての通貨"
        }));
        let inputs = hidden_inputs(&html);
        assert_eq!(
            inputs,
            vec![
                ("action".into(), "approve".into()),
                ("flow_id".into(), flow.id.clone())
            ]
        );
        let mut encoded = reqwest::Url::parse("https://vc.example/").unwrap();
        encoded.query_pairs_mut().extend_pairs(&inputs);
        let approved = post(
            &f.app,
            &flow,
            encoded.query().unwrap(),
            &[("origin", &links().site_url)],
        )
        .await;
        assert_eq!(approved.status(), 303);
        assert!(
            approved.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
        let url = reqwest::Url::parse(approved.headers()[LOCATION].to_str().unwrap()).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["state"], CLIENT_STATE);
        assert_eq!(pairs["guild_id"], MONEY_GUILD.to_string());
        assert_eq!(pairs["scope"], "vc.issue");
        assert_eq!(pairs["y"], "two");
        let exchange = json!({"grant_type":"authorization_code", "client_id":f.client, "redirect_uri":REDIRECT, "code":pairs["code"]});
        let response = f
            .app
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
        assert_eq!(response.status(), 200);
        let token: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let stored: Vec<i64> = sqlx::query_scalar("SELECT r.currency_id FROM grant_resources r JOIN access_tokens t ON t.grant_id=r.grant_id WHERE t.token_id=$1")
            .bind(uuid::Uuid::parse_str(token["access_token"].as_str().unwrap()).unwrap()).fetch_all(&pool).await.unwrap();
        assert_eq!(stored, if restricted { vec![1] } else { vec![] });
        assert_eq!(approve(&f.app, &flow).await.status(), 401);
    }
    for table in [
        "SELECT count(*) FROM discord_users",
        "SELECT count(*) FROM user_access_tokens",
        "SELECT count(*) FROM browser_authorizations",
    ] {
        let count: i64 = sqlx::query_scalar(table).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 0, "no persistent login credentials: {table}");
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn independent_flows_require_their_own_browser_and_callback_once(pool: PgPool) {
    let f = fixture(&pool).await;
    let a = start(&f, &[]).await;
    let b = start(&f, &[]).await;
    assert_ne!(a.id, b.id);
    assert_eq!(
        approve(&f.app, &a).await.status(),
        401,
        "cannot approve before Discord"
    );
    for cookie in [
        None,
        Some(b.cookie.as_str()),
        Some("_virtualcrypto_session=legacy"),
    ] {
        assert_eq!(
            callback(&f.app, &a, cookie, "text/html").await.status(),
            401
        );
    }
    let both = format!("{}; {}", a.cookie, b.cookie);
    for flow in [&a, &b] {
        let response = callback(&f.app, flow, Some(&both), "application/json").await;
        assert_eq!(response.status(), 200);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["flow_id"], flow.id);
        assert_eq!(body["client_name"], NAME);
        assert_eq!(body["state"], CLIENT_STATE);
        assert_eq!(
            callback(&f.app, flow, Some(&both), "application/json")
                .await
                .status(),
            401
        );
    }
    let substituted = Flow {
        id: a.id.clone(),
        cookie: b.cookie.clone(),
    };
    assert_eq!(approve(&f.app, &substituted).await.status(), 401);
    assert_eq!(approve(&f.app, &a).await.status(), 303);
    assert_eq!(approve(&f.app, &b).await.status(), 303);
    assert_eq!(count_codes(&pool).await, 2);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn forged_post_fields_cannot_change_the_saved_request(pool: PgPool) {
    let f = fixture(&pool).await;
    let resource = format!("{}/api/v2/currencies/1", links().site_url);
    let flow = start(&f, &[resource]).await;
    assert_eq!(
        callback(&f.app, &flow, Some(&flow.cookie), "text/html")
            .await
            .status(),
        200
    );
    let response = post(&f.app, &flow, &format!("{}&client_id=other&redirect_uri=https://attacker.example/&scope=vc.pay&guild_id=42&resource=https://attacker.example/&state=changed", form(&flow)), &[("sec-fetch-site","same-origin")]).await;
    assert_eq!(response.status(), 303);
    let url = reqwest::Url::parse(response.headers()[LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("app.example"));
    assert_eq!(
        url.query_pairs().find(|(k, _)| k == "state").unwrap().1,
        CLIENT_STATE
    );
    let stored: (i64, Vec<i64>, Vec<String>) =
        sqlx::query_as("SELECT guild_id, resources, scopes::text[] FROM authorization_codes")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, (MONEY_GUILD, vec![1], vec!["vc.issue".into()]));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approval_requires_same_origin_and_a_live_single_use_flow(pool: PgPool) {
    let f = fixture(&pool).await;
    for fields in [
        vec![],
        vec![("origin", "https://attacker.example")],
        vec![("sec-fetch-site", "same-site")],
        vec![
            ("origin", "null"),
            ("referer", "https://vcrypto.sumidora.com/"),
        ],
        vec![
            ("sec-fetch-site", "same-origin"),
            ("origin", "https://attacker.example"),
        ],
    ] {
        let flow = start(&f, &[]).await;
        assert_eq!(
            callback(&f.app, &flow, Some(&flow.cookie), "text/html")
                .await
                .status(),
            200
        );
        assert_eq!(
            post(&f.app, &flow, &form(&flow), &fields).await.status(),
            403
        );
    }
    assert_eq!(count_codes(&pool).await, 0);
    let flow = start(&f, &[]).await;
    let (a, b) = tokio::join!(
        callback(&f.app, &flow, Some(&flow.cookie), "text/html"),
        callback(&f.app, &flow, Some(&flow.cookie), "text/html")
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 401]);
    let (a, b) = tokio::join!(approve(&f.app, &flow), approve(&f.app, &flow));
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [303, 401]);
    assert_eq!(count_codes(&pool).await, 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn expired_requests_and_expired_consent_cannot_be_used(pool: PgPool) {
    let f = fixture(&pool).await;
    for verified in [false, true] {
        let flow = start(&f, &[]).await;
        if verified {
            assert_eq!(
                callback(&f.app, &flow, Some(&flow.cookie), "text/html")
                    .await
                    .status(),
                200
            );
        }
        sqlx::query(
            "UPDATE browser_authorizations SET expires=now()-interval '1 second' WHERE id=$1",
        )
        .bind(uuid::Uuid::parse_str(&flow.id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            callback(&f.app, &flow, Some(&flow.cookie), "text/html")
                .await
                .status(),
            401
        );
        assert_eq!(approve(&f.app, &flow).await.status(), 401);
    }
    assert_eq!(count_codes(&pool).await, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approval_rechecks_guild_permissions_and_client_registration(pool: PgPool) {
    let f = fixture(&pool).await;
    let flow = start(&f, &[]).await;
    assert_eq!(
        callback(&f.app, &flow, Some(&flow.cookie), "text/html")
            .await
            .status(),
        200
    );
    let denied = vc_api::router(state(
        pool.clone(),
        FakeDiscord::with_member(MONEY_USER2, &[], &[]),
    ));
    assert_eq!(approve(&denied, &flow).await.status(), 400);
    assert_eq!(count_codes(&pool).await, 0);
    sqlx::query("DELETE FROM redirect_uris WHERE application_id=$1")
        .bind(f.application)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(approve(&f.app, &flow).await.status(), 400);
    assert_eq!(count_codes(&pool).await, 0);
    // Failed issuance rolls consumption back; after restoring registration it can be approved once.
    sqlx::query("INSERT INTO redirect_uris(application_id,redirect_uri,inserted_at,updated_at) VALUES($1,$2,now(),now())")
        .bind(f.application).bind(REDIRECT).execute(&pool).await.unwrap();
    assert_eq!(approve(&f.app, &flow).await.status(), 303);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn invalid_requests_do_not_redirect_to_unverified_clients_or_create_flows(pool: PgPool) {
    let f = fixture(&pool).await;
    for uri in [
        "/oauth2/authorize?response_type=token".to_owned(),
        uri("unregistered", &[]),
        uri(&f.client, &[]).replace(&format!("guild_id={MONEY_GUILD}"), "guild_id=invalid"),
        uri(&f.client, &[]).replace("redirect_uri=https", "redirect_uri=otherhttps"),
    ] {
        let response = get(&f.app, &uri, None, "text/html").await;
        assert_eq!(response.status(), 400, "{uri}");
        assert!(!response.headers().contains_key(LOCATION));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM browser_authorizations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_or_denied_discord_callbacks_cannot_be_replayed(pool: PgPool) {
    let f = fixture(&pool).await;
    let flow = start(&f, &[]).await;
    let rejected = get(
        &f.app,
        &format!("/callback/discord?state={}&error=access_denied", flow.id),
        Some(&flow.cookie),
        "text/html",
    )
    .await;
    assert_eq!(rejected.status(), 400);
    assert_eq!(
        callback(&f.app, &flow, Some(&flow.cookie), "text/html")
            .await
            .status(),
        401
    );
    assert_eq!(approve(&f.app, &flow).await.status(), 401);
    assert_eq!(count_codes(&pool).await, 0);
}
