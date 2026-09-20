//! `GET /api/v2/contracts/{id}/payments`: what a contract has paid out, read
//! back.
//!
//! A charge is not a receipt until it can be read again — an application has to
//! reconcile what it billed, and the person who paid has to be able to see where
//! their money went. What a row *is* matters as much as what it says: one
//! payment draws on as many parties as it needs and writes one row per party
//! drawn on, so a statement of a multi-party contract has several rows for one
//! charge, and these pin that as the shape rather than as an accident.

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
const BOB: i32 = 3;
const BOB_DISCORD_ID: i64 = 100_000_000_000_000_002;
const STRANGER: i32 = 4;
const STRANGER_DISCORD_ID: i64 = 100_000_000_000_000_003;
const RECEIVER_DISCORD_ID: i64 = 500_000_000_000_000_002;
const GUILD: i64 = 900_000_000_000_000_001;

struct Fixture {
    token: String,
    contract: i64,
}

/// One party's contract, or two when the test needs a payment that draws on more
/// than one — and a fixed receiver in both, so the money's path is never in
/// question.
async fn fixture(pool: &PgPool, parties: &[(i32, i64, i64)]) -> Fixture {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, ALICE, ALICE_DISCORD_ID).await;
    insert_user(pool, BOB, BOB_DISCORD_ID).await;
    insert_user(pool, STRANGER, STRANGER_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;

    for (account, _, amount) in parties {
        insert_asset(pool, *account, 1, 1_000 + amount).await;
    }

    let application = insert_application(pool, OWNER_DISCORD_ID, "a metered service").await;
    let account = account_of(pool, application).await;
    let token = mint_app(pool, account, &["vc.contract"]).await;

    let named: Vec<Value> = parties
        .iter()
        .map(|(_, discord_id, amount)| {
            json!({ "discord_id": discord_id.to_string(), "amount": amount.to_string() })
        })
        .collect();

    let created = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        &token,
        json!({
            "unit": "nyan",
            "parties": named,
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
        }),
    )
    .await;

    assert_eq!(created.status, 201, "body: {}", created.body);

    let contract: i64 = created.body["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a number");

    for (account, _, _) in parties {
        let token = mint(pool, *account, &[]).await;

        let approved = send(
            vc_api::router(state(pool.clone(), fake())),
            "POST",
            &format!("/api/v2/contracts/{contract}/approval"),
            &token,
            Value::Null,
        )
        .await;

        assert_eq!(approved.status, 200, "body: {}", approved.body);
    }

    Fixture { token, contract }
}

async fn send(app: axum::Router, method: &str, uri: &str, token: &str, body: Value) -> Response {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json")
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

async fn charge(pool: &PgPool, fixture: &Fixture, amount: i64) -> Response {
    send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        &fixture.token,
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": amount.to_string(),
        }),
    )
    .await
}

async fn statement(pool: &PgPool, fixture: &Fixture, token: &str) -> Response {
    send(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        token,
        Value::Null,
    )
    .await
}

/// Two charges are two rows, newest first, and each one says which party paid,
/// how much, where it went and when.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_statement_names_the_charges_newest_first(pool: PgPool) {
    let fixture = fixture(&pool, &[(ALICE, ALICE_DISCORD_ID, 100)]).await;

    for amount in [25, 30] {
        let charged = charge(&pool, &fixture, amount).await;

        assert_eq!(charged.status, 201, "body: {}", charged.body);
    }

    let listed = statement(&pool, &fixture, &fixture.token).await;

    assert_eq!(listed.status, 200, "body: {}", listed.body);

    let rows = listed.body.as_array().expect("an array");
    assert_eq!(rows.len(), 2, "body: {}", listed.body);

    assert_eq!(rows[0]["amount"], "30", "the newest first");
    assert_eq!(rows[1]["amount"], "25");

    for row in rows {
        assert_eq!(row["discord_id"], ALICE_DISCORD_ID.to_string());
        assert_eq!(row["receiver_discord_id"], RECEIVER_DISCORD_ID.to_string());
        assert!(
            row["id"].as_str().is_some_and(|id| !id.is_empty()),
            "a row to point at: {row}"
        );
        assert!(
            row["time"]
                .as_str()
                .is_some_and(|time| time.contains('T') && time.ends_with('Z')),
            "the family's timestamp: {row}"
        );
    }
}

/// One payment that draws on two parties is two rows, with the party each slice
/// came out of: the statement says whose remainder paid for what, and the slices
/// are in the order they were drawn.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn one_payment_of_two_parties_is_two_rows(pool: PgPool) {
    let fixture = fixture(
        &pool,
        &[(ALICE, ALICE_DISCORD_ID, 100), (BOB, BOB_DISCORD_ID, 50)],
    )
    .await;

    // Oldest approval first: Alice's hundred and then twenty of Bob's.
    let charged = charge(&pool, &fixture, 120).await;

    assert_eq!(charged.status, 201, "body: {}", charged.body);
    assert_eq!(
        charged.body["party_remaining"],
        Value::Null,
        "a draw across the parties is no single one's"
    );

    let listed = statement(&pool, &fixture, &fixture.token).await;
    let rows = listed.body.as_array().expect("an array");

    assert_eq!(rows.len(), 2, "body: {}", listed.body);

    // Newest first, and a payment's rows are written in the order it drew them —
    // so the later slice is the one on top.
    assert_eq!(rows[0]["discord_id"], BOB_DISCORD_ID.to_string());
    assert_eq!(rows[0]["amount"], "20");
    assert_eq!(rows[1]["discord_id"], ALICE_DISCORD_ID.to_string());
    assert_eq!(rows[1]["amount"], "100");
}

/// The same readers as the contract itself, and a stranger is answered as a
/// contract that is not there.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn only_its_own_people_can_read_it(pool: PgPool) {
    let fixture = fixture(&pool, &[(ALICE, ALICE_DISCORD_ID, 100)]).await;
    let alice = mint(&pool, ALICE, &[]).await;
    let stranger = mint(&pool, STRANGER, &[]).await;

    charge(&pool, &fixture, 25).await;

    for token in [&fixture.token, &alice] {
        let listed = statement(&pool, &fixture, token).await;

        assert_eq!(listed.status, 200, "body: {}", listed.body);
        assert_eq!(listed.body.as_array().map(Vec::len), Some(1));
    }

    let refused = statement(&pool, &fixture, &stranger).await;

    assert_eq!(refused.status, 404, "body: {}", refused.body);
}

/// A row the column does not name is not in anybody's statement: payments made
/// before the ledger knew about contracts, and every ordinary transfer since.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payment_that_names_no_contract_is_not_in_it(pool: PgPool) {
    let fixture = fixture(&pool, &[(ALICE, ALICE_DISCORD_ID, 100)]).await;

    charge(&pool, &fixture, 25).await;

    // The shape an ordinary transfer writes: the same sender, the same receiver,
    // the same currency, and no contract.
    sqlx::query!(
        "INSERT INTO currency_payment_histories
             (amount, sender_id, receiver_id, currency_id, time, inserted_at, updated_at)
         SELECT 5, alice.id, receiver.id, 1, now(), now(), now()
           FROM users alice, users receiver
          WHERE alice.discord_id = $1 AND receiver.discord_id = $2",
        ALICE_DISCORD_ID,
        RECEIVER_DISCORD_ID
    )
    .execute(&pool)
    .await
    .expect("an ordinary payment");

    let listed = statement(&pool, &fixture, &fixture.token).await;
    let rows = listed.body.as_array().expect("an array");

    assert_eq!(rows.len(), 1, "body: {}", listed.body);
    assert_eq!(rows[0]["amount"], "25");
}
