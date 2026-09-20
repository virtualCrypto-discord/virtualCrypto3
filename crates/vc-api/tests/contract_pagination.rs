//! The contract lists, paged.
//!
//! `limit` is what turns one of them into a page, `next` and `on_next` are what
//! continue it, and a page that came back full carries a `link` header — the same
//! three things `GET /api/v2/users/@me/claims` takes and answers with, because
//! the machinery is `routes/pagination.rs` and these pin that these lists are
//! wired to it. The difference is the default: an absent `limit` still means
//! every row for the two lists that existed before they could be paged, and a
//! page for the statement, whose rows are written by every use an application
//! bills for.

mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    Response, account_of, fake, insert_application, insert_asset, insert_currency, insert_user,
    mint, mint_app, state,
};
use tower::ServiceExt;

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const ALICE: i32 = 2;
const ALICE_DISCORD_ID: i64 = 100_000_000_000_000_001;
const RECEIVER_DISCORD_ID: i64 = 500_000_000_000_000_002;
const GUILD: i64 = 900_000_000_000_000_001;

/// A quota worth a page of charges, at one a use.
const QUOTA: i64 = 100;

struct Fixture {
    token: String,
}

async fn fixture(pool: &PgPool) -> Fixture {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, ALICE, ALICE_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, ALICE, 1, 1_000).await;

    let application = insert_application(pool, OWNER_DISCORD_ID, "a metered service").await;
    let account = account_of(pool, application).await;
    let token = mint_app(pool, account, &["vc.contract"]).await;

    Fixture { token }
}

async fn send(app: axum::Router, method: &str, uri: &str, token: &str, body: Value) -> Response {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json")
        .header("host", "localhost:4000")
        .header("authorization", format!("Bearer {token}"));

    if !body.is_null() {
        builder = builder.header("content-type", "application/json");
    }

    let request = builder
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("encode body"),
        ))
        .expect("request");

    let response = app.oneshot(request).await.expect("router response");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };

    Response {
        status,
        headers,
        body,
    }
}

/// One contract naming Alice, locked and ready to be charged.
async fn open(pool: &PgPool, fixture: &Fixture) -> i64 {
    let created = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        &fixture.token,
        json!({
            "unit": "nyan",
            "parties": [{
                "discord_id": ALICE_DISCORD_ID.to_string(),
                "amount": QUOTA.to_string(),
            }],
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
        }),
    )
    .await;

    assert_eq!(created.status, 201, "body: {}", created.body);

    created.body["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a number")
}

async fn approved(pool: &PgPool, contract: i64) {
    let alice = mint(pool, ALICE, &[]).await;

    let decided = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{contract}/approval"),
        &alice,
        Value::Null,
    )
    .await;

    assert_eq!(decided.status, 200, "body: {}", decided.body);
}

async fn charged(pool: &PgPool, fixture: &Fixture, contract: i64, amount: i64) {
    let charged = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{contract}/payments"),
        &fixture.token,
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": amount.to_string(),
        }),
    )
    .await;

    assert_eq!(charged.status, 201, "body: {}", charged.body);
}

async fn list(pool: &PgPool, fixture: &Fixture, query: &str) -> Response {
    let uri = if query.is_empty() {
        "/api/v2/contracts".to_owned()
    } else {
        format!("/api/v2/contracts?{query}")
    };

    send(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        &uri,
        &fixture.token,
        Value::Null,
    )
    .await
}

async fn statement(pool: &PgPool, fixture: &Fixture, contract: i64, query: &str) -> Response {
    let uri = if query.is_empty() {
        format!("/api/v2/contracts/{contract}/payments")
    } else {
        format!("/api/v2/contracts/{contract}/payments?{query}")
    };

    send(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        &uri,
        &fixture.token,
        Value::Null,
    )
    .await
}

/// The ids a list answered with, newest first.
fn ids(response: &Response) -> Vec<i64> {
    response
        .body
        .as_array()
        .expect("an array")
        .iter()
        .map(|row| {
            row["id"]
                .as_str()
                .expect("an id")
                .parse()
                .expect("a number")
        })
        .collect()
}

/// The cursor a `link` header points at, which is what a client follows.
fn next_of(response: &Response) -> Option<i64> {
    let link = response.headers.get("link")?.to_str().ok()?;

    link.split_once("next=")?
        .1
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Without a `limit` a list is what it always was: every row.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_absent_limit_is_every_contract(pool: PgPool) {
    let fixture = fixture(&pool).await;

    for _ in 0..3 {
        open(&pool, &fixture).await;
    }

    let response = list(&pool, &fixture, "").await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(ids(&response).len(), 3, "body: {}", response.body);
    assert_eq!(next_of(&response), None, "nothing to continue to");
}

/// A page that came back full is continued by the header it carries, and the
/// page after it is the rest.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_full_page_links_to_the_next_one(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let mut all = Vec::new();
    for _ in 0..3 {
        all.push(open(&pool, &fixture).await);
    }

    let first = list(&pool, &fixture, "limit=2").await;

    assert_eq!(first.status, 200, "body: {}", first.body);
    assert_eq!(ids(&first), vec![all[2], all[1]], "newest first");

    let link = first
        .headers
        .get("link")
        .and_then(|value| value.to_str().ok())
        .expect("a full page advertises the next one")
        .to_owned();

    assert!(
        link.starts_with("<http://localhost:4000/api/v2/contracts?limit=2&next="),
        "the header a client can follow: {link}"
    );

    let next = next_of(&first).expect("a cursor");
    assert_eq!(next, all[1], "the last row of the page");

    let rest = list(&pool, &fixture, &format!("limit=2&next={next}")).await;

    assert_eq!(ids(&rest), vec![all[0]]);
    assert_eq!(next_of(&rest), None, "a short page is the end");
}

/// `next` resumes after the row it names and `on_next` resumes at it — the same
/// two the claim list takes, with the same meaning.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn next_excludes_its_row_and_on_next_includes_it(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let mut all = Vec::new();
    for _ in 0..3 {
        all.push(open(&pool, &fixture).await);
    }

    let after = list(&pool, &fixture, &format!("next={}", all[1])).await;
    assert_eq!(ids(&after), vec![all[0]], "older than the cursor");

    let from = list(&pool, &fixture, &format!("on_next={}", all[1])).await;
    assert_eq!(ids(&from), vec![all[1], all[0]], "from the cursor on");

    let both = list(
        &pool,
        &fixture,
        &format!("next={}&on_next={}", all[1], all[1]),
    )
    .await;
    assert_eq!(both.status, 400, "body: {}", both.body);
    assert_eq!(both.body["error_description"], "invalid_cursor");
}

/// A statement is paged whether or not anyone asked, because its rows are written
/// by every use an application bills for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_statement_pages_by_default(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let contract = open(&pool, &fixture).await;
    approved(&pool, contract).await;

    for _ in 0..51 {
        charged(&pool, &fixture, contract, 1).await;
    }

    let first = statement(&pool, &fixture, contract, "").await;

    assert_eq!(first.status, 200, "body: {}", first.body);
    assert_eq!(ids(&first).len(), 50, "the default page");
    assert_eq!(next_of(&first), Some(ids(&first)[49]), "the last row of it");

    let next = next_of(&first).expect("a cursor");
    let rest = statement(&pool, &fixture, contract, &format!("next={next}")).await;

    assert_eq!(ids(&rest).len(), 1, "the rest of it");
    assert_eq!(next_of(&rest), None);
}

/// A negative limit is a client's typo, and it is answered as one: Postgres would
/// answer it with an error of its own, which is a 500 for something the caller can
/// fix without help. Zero is a page of nothing rather than a mistake, and stays
/// one.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_negative_limit_is_a_client_error(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let contract = open(&pool, &fixture).await;

    for query in ["limit=-1", "limit=-100"] {
        let listed = list(&pool, &fixture, query).await;

        assert_eq!(listed.status, 400, "body: {}", listed.body);
        assert_eq!(listed.body["error_description"], "invalid_limit");

        let entries = statement(&pool, &fixture, contract, query).await;

        assert_eq!(entries.status, 400, "body: {}", entries.body);
        assert_eq!(entries.body["error_description"], "invalid_limit");
    }

    let empty = list(&pool, &fixture, "limit=0").await;

    assert_eq!(empty.status, 200, "body: {}", empty.body);
    assert_eq!(ids(&empty).len(), 0, "a page of nothing is still a page");
    assert_eq!(next_of(&empty), None);
}
