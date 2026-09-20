//! Whose use a payment bills, and the one thing a fixed receiver gives way to.
//!
//! A contract is a pot: the parties' remainders are summed, and a payment draws
//! oldest-approval-first across them. That is right for an escrow and wrong for
//! metering — an application that is billing Alice must not spend Bob's part for
//! it — so `party_discord_id` is how a payment says whose use it is.
//!
//! The same field is what makes a return possible: money a party locked may go
//! back to that party even when the contract fixes its receiver, because paying
//! a party their own remainder is not a spend to somebody else. What it is not
//! is a way to move one party's remainder to another person, which is why the
//! exemption holds only when the receiver *is* the party being drawn on.

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
const RECEIVER_DISCORD_ID: i64 = 500_000_000_000_000_002;
const GUILD: i64 = 900_000_000_000_000_001;

/// The application, two approved parties, and the contract between them: Alice
/// locked 100, Bob 50, and every charge may only go to the operator.
struct Fixture {
    token: String,
    contract: i64,
}

async fn fixture(pool: &PgPool) -> Fixture {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, ALICE, ALICE_DISCORD_ID).await;
    insert_user(pool, BOB, BOB_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, ALICE, 1, 1_000).await;
    insert_asset(pool, BOB, 1, 1_000).await;

    let application = insert_application(pool, OWNER_DISCORD_ID, "a metered service").await;
    let account = account_of(pool, application).await;
    let token = mint_app(pool, account, &["vc.contract"]).await;

    let created = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        &token,
        json!({
            "unit": "nyan",
            "parties": [party(ALICE_DISCORD_ID, 100), party(BOB_DISCORD_ID, 50)],
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

    for (account, discord_id) in [(ALICE, ALICE_DISCORD_ID), (BOB, BOB_DISCORD_ID)] {
        let token = mint(pool, account, &[]).await;

        let approved = send(
            vc_api::router(state(pool.clone(), fake())),
            "POST",
            &format!("/api/v2/contracts/{contract}/approval"),
            &token,
            Value::Null,
        )
        .await;

        assert_eq!(approved.status, 200, "{}: {}", discord_id, approved.body);
    }

    Fixture { token, contract }
}

fn party(discord_id: i64, amount: i64) -> Value {
    json!({ "discord_id": discord_id.to_string(), "amount": amount.to_string() })
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

/// A charge that names the party it bills, or nobody when the body says nothing.
async fn charge(
    pool: &PgPool,
    fixture: &Fixture,
    party: Option<i64>,
    receiver: i64,
    amount: i64,
) -> Response {
    let mut body = json!({
        "receiver_discord_id": receiver.to_string(),
        "amount": amount.to_string(),
    });

    if let Some(party) = party {
        body["party_discord_id"] = json!(party.to_string());
    }

    send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        &fixture.token,
        body,
    )
    .await
}

/// What each party has left of their approval, as the contract answers it.
async fn remainder(pool: &PgPool, fixture: &Fixture, discord_id: i64) -> i64 {
    let response = send(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        &format!("/api/v2/contracts/{}", fixture.contract),
        &fixture.token,
        Value::Null,
    )
    .await;

    let named = discord_id.to_string();

    response.body["parties"]
        .as_array()
        .expect("parties")
        .iter()
        .find(|party| party["discord_id"].as_str() == Some(named.as_str()))
        .expect("the party")["remaining"]
        .as_str()
        .expect("a number")
        .parse()
        .expect("a number")
}

async fn balance(pool: &PgPool, account: i32) -> i64 {
    sqlx::query_scalar!(
        "SELECT amount FROM assets WHERE user_id = $1 AND currency_id = $2",
        i64::from(account),
        i64::from(1)
    )
    .fetch_optional(pool)
    .await
    .expect("a balance")
    .flatten()
    .unwrap_or(0)
}

/// A charge that names a party draws on that party and nobody else, which is what
/// makes per-person billing possible at all.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_charge_draws_on_the_party_it_names(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let charged = charge(
        &pool,
        &fixture,
        Some(BOB_DISCORD_ID),
        RECEIVER_DISCORD_ID,
        30,
    )
    .await;

    assert_eq!(charged.status, 201, "body: {}", charged.body);
    assert_eq!(
        charged.body["remaining"], "120",
        "the contract, which is both parties' rest"
    );
    assert_eq!(
        charged.body["party_remaining"], "20",
        "and the party it named, which is the one a per-person biller wants"
    );

    assert_eq!(remainder(&pool, &fixture, BOB_DISCORD_ID).await, 20);
    assert_eq!(
        remainder(&pool, &fixture, ALICE_DISCORD_ID).await,
        100,
        "Alice was not billed for Bob's use"
    );
}

/// A party the contract does not name is not the same nothing as a party who has
/// spent their part: one is a body that names somebody wrong, the other is a
/// quota that is gone.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_stranger_is_not_a_party(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let refused = charge(&pool, &fixture, Some(999_000_000), RECEIVER_DISCORD_ID, 10).await;

    assert_eq!(refused.status, 400, "body: {}", refused.body);
    assert_eq!(refused.body["error_description"], "not_a_party");
}

/// A named party is a hard bound even when the contract as a whole holds more:
/// the application cannot spend Bob's part for a bill Alice's quota does not
/// cover.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_party_with_nothing_left_is_not_enough(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let refused = charge(
        &pool,
        &fixture,
        Some(ALICE_DISCORD_ID),
        RECEIVER_DISCORD_ID,
        120,
    )
    .await;

    assert_eq!(refused.status, 409, "body: {}", refused.body);
    assert_eq!(refused.body["error_info"], "not_enough_amount");
    assert_eq!(remainder(&pool, &fixture, ALICE_DISCORD_ID).await, 100);
}

/// Giving a party their own remainder back is not a spend: it is allowed even
/// where the receiver is fixed, and it is how a use that should not have been
/// billed is undone.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_return_gives_a_party_their_remainder_back(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let charged = charge(
        &pool,
        &fixture,
        Some(ALICE_DISCORD_ID),
        RECEIVER_DISCORD_ID,
        30,
    )
    .await;

    assert_eq!(charged.status, 201, "body: {}", charged.body);
    assert_eq!(balance(&pool, ALICE).await, 900);

    let returned = charge(
        &pool,
        &fixture,
        Some(ALICE_DISCORD_ID),
        ALICE_DISCORD_ID,
        30,
    )
    .await;

    assert_eq!(returned.status, 201, "body: {}", returned.body);
    assert_eq!(returned.body["remaining"], "90", "out of the contract now");
    assert_eq!(
        returned.body["party_remaining"], "40",
        "and what the party may still be billed for"
    );
    assert_eq!(balance(&pool, ALICE).await, 930, "hers came back");
    assert_eq!(
        remainder(&pool, &fixture, ALICE_DISCORD_ID).await,
        40,
        "and can no longer be billed"
    );
}

/// The exemption is the receiver being the party drawn on, not a door out of the
/// fixed receiver: Alice's remainder may not be paid to Bob.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_return_may_not_pay_somebody_else(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let refused = charge(&pool, &fixture, Some(ALICE_DISCORD_ID), BOB_DISCORD_ID, 30).await;

    assert_eq!(refused.status, 400, "body: {}", refused.body);
    assert_eq!(refused.body["error_description"], "receiver_is_fixed");
    assert_eq!(balance(&pool, BOB).await, 950, "still locked");
}

/// The party is an id like every other one here, and is read the way they are.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_party_is_read_like_every_other_id(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let unreadable = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        &fixture.token,
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": "10",
            "party_discord_id": "nonsense",
        }),
    )
    .await;

    assert_eq!(unreadable.status, 400, "body: {}", unreadable.body);
    assert_eq!(
        unreadable.body["error_description"],
        "invalid_format_of_party_discord_id"
    );

    let wrong_type = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        &fixture.token,
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": "10",
            "party_discord_id": 100,
        }),
    )
    .await;

    assert_eq!(wrong_type.status, 400, "body: {}", wrong_type.body);
    assert_eq!(
        wrong_type.body["error_description"],
        "invalid_type_of_variable"
    );
}
