//! Metered spending: one approved quota, drawn down a use at a time.
//!
//! A pay-per-use application does not spend a contract once — it charges as it
//! works, and what is left is what it has not billed for yet. This pins what
//! makes that safe: repeated partial draws against the same approval, the
//! remainder counting down to exactly nothing, and the refusal that ends the
//! run.
//!
//! `tests/contracts.rs` has the single partial payment and the refusals around
//! it; this is the same endpoint paid against many times, which is the shape
//! `crates/vc-demo-app/src/bin/demo-billing.rs` is built on.

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
const SUBSCRIBER: i32 = 2;
const SUBSCRIBER_DISCORD_ID: i64 = 100_000_000_000_000_001;
const OPERATOR_DISCORD_ID: i64 = 500_000_000_000_000_002;
const GUILD: i64 = 900_000_000_000_000_001;

/// What the subscriber locks, which is the whole of what may be billed.
const QUOTA: i64 = 100;

/// A month in seconds: a quota is per period, so the contract has a deadline and
/// the subscriber cannot take the remainder back while it runs.
const MONTH: i64 = 30 * 24 * 60 * 60;

async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> Response {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json");

    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }

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

/// The application that bills, the subscriber whose quota it draws on, and the
/// contract the two of them have between them.
struct Fixture {
    token: String,
    subscriber: String,
    contract: i64,
}

/// The ask: one subscriber, what they lock, a month of it, and the receiver
/// fixed to the operator's own account — which is what the subscriber is
/// trusting, and what every charge below has to name.
fn asked() -> Value {
    json!({
        "unit": "nyan",
        "parties": [{
            "discord_id": SUBSCRIBER_DISCORD_ID.to_string(),
            "amount": QUOTA.to_string(),
        }],
        "receiver_discord_id": OPERATOR_DISCORD_ID.to_string(),
        "expires_in": MONTH,
    })
}

async fn fixture(pool: &PgPool) -> Fixture {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, SUBSCRIBER, SUBSCRIBER_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, SUBSCRIBER, 1, 1_000).await;

    let application = insert_application(pool, OWNER_DISCORD_ID, "a metered service").await;
    let account = account_of(pool, application).await;
    let token = mint_app(pool, account, &["vc.contract"]).await;
    let contract = id_of(pool, &token, asked()).await;

    Fixture {
        subscriber: mint(pool, SUBSCRIBER, &[]).await,
        token,
        contract,
    }
}

async fn id_of(pool: &PgPool, token: &str, body: Value) -> i64 {
    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        Some(token),
        body,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);

    response.body["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a number")
}

/// One use's charge: a partial spend of the same approval.
async fn charge(pool: &PgPool, token: &str, contract: i64, amount: i64) -> Response {
    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{contract}/payments"),
        Some(token),
        json!({
            "receiver_discord_id": OPERATOR_DISCORD_ID.to_string(),
            "amount": amount.to_string(),
        }),
    )
    .await
}

async fn balance(pool: &PgPool, account: i32, currency: i64) -> i64 {
    sqlx::query_scalar!(
        "SELECT amount FROM assets WHERE user_id = $1 AND currency_id = $2",
        i64::from(account),
        currency
    )
    .fetch_optional(pool)
    .await
    .expect("a balance")
    .flatten()
    .unwrap_or(0)
}

async fn status_of(pool: &PgPool, id: i64) -> String {
    sqlx::query_scalar!("SELECT status FROM contracts WHERE id = $1", id)
        .fetch_one(pool)
        .await
        .expect("the contract")
}

async fn histories(pool: &PgPool) -> i64 {
    sqlx::query_scalar!("SELECT count(*) AS \"count!\" FROM currency_payment_histories")
        .fetch_one(pool)
        .await
        .expect("the ledger")
}

/// A quota is drawn down one use at a time: the same approval is spent in part
/// over and over, and the last draw is the one that empties it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_quota_is_drawn_down_a_use_at_a_time(pool: PgPool) {
    let fixture = fixture(&pool).await;

    // Approving is locking, and with one party named it makes the contract
    // active at once — the state the application waits for before it bills
    // anything.
    let approval = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/approval", fixture.contract),
        Some(&fixture.subscriber),
        Value::Null,
    )
    .await;

    assert_eq!(approval.status, 200, "body: {}", approval.body);
    assert_eq!(approval.body["status"], "active");
    assert_eq!(approval.body["remaining"], QUOTA.to_string());
    assert_eq!(balance(&pool, SUBSCRIBER, 1).await, 900, "locked out of it");

    // Three uses, three charges: two of a quarter, then all of what is left.
    for (use_number, amount, left) in [(1, 25, 75), (2, 25, 50), (3, 50, 0)] {
        let charged = charge(&pool, &fixture.token, fixture.contract, amount).await;

        assert_eq!(charged.status, 201, "use {use_number}: {}", charged.body);
        assert_eq!(charged.body["amount"], amount.to_string());
        assert_eq!(charged.body["remaining"], left.to_string());
        assert_eq!(charged.body["unit"], "nyan");
    }

    // The quota is spent, so the next use is refused: what ends the run is the
    // money, not the application's own accounting.
    let refused = charge(&pool, &fixture.token, fixture.contract, 1).await;

    assert_eq!(refused.status, 409, "body: {}", refused.body);
    assert_eq!(refused.body["error_info"], "not_enough_amount");
    assert_eq!(
        status_of(&pool, fixture.contract).await,
        "active",
        "spent, not over: the deadline is what ends it"
    );

    // What the approval locked is what the operator was paid, a draw at a time —
    // the subscriber's balance went at the approval, not at each use — and the
    // ledger has one row per charge, beside the lock the approval itself wrote.
    assert_eq!(balance(&pool, SUBSCRIBER, 1).await, 900);

    let operator = vc_core::user::find_by_discord_id(&pool, OPERATOR_DISCORD_ID)
        .await
        .expect("a lookup")
        .expect("the receiver");

    assert_eq!(balance(&pool, operator.id, 1).await, QUOTA);
    assert_eq!(
        histories(&pool).await,
        4,
        "the lock (the approval) and one row per draw"
    );
}
