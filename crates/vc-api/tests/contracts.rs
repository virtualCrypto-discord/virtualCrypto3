//! `/api/v2/contracts`: an application operating a user's currency, on the
//! strength of the parties' approvals.
//!
//! Additions, not ports: the Elixir's contract page is a mockup with no endpoint
//! behind it, so what these pin is the design in `docs/contracts.md` — locking
//! on approval, everyone-approved making it active, the application spending
//! what is left, and a refusal or a withdrawal ending it and sending what is
//! left home.

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
const GUILD: i64 = 900_000_000_000_000_001;

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

/// An application that may write contracts, and the accounts and balances its
/// contracts are about.
struct Fixture {
    application: i64,
    token: String,
}

async fn fixture(pool: &PgPool) -> Fixture {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, ALICE, ALICE_DISCORD_ID).await;
    insert_user(pool, BOB, BOB_DISCORD_ID).await;
    insert_user(pool, STRANGER, STRANGER_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, ALICE, 1, 1_000).await;
    insert_asset(pool, BOB, 1, 200).await;

    let application = insert_application(pool, OWNER_DISCORD_ID, "an application").await;
    let account = account_of(pool, application).await;
    let token = mint_app(pool, account, &["vc.contract"]).await;

    Fixture { application, token }
}

/// A user token, which is what a party answers with. The scopes are empty on
/// purpose: being named is what lets a user answer, not what the token carries.
async fn user_token(pool: &PgPool, account: i32) -> String {
    mint(pool, account, &[]).await
}

fn asked(unit: &str, parties: Value) -> Value {
    json!({ "unit": unit, "parties": parties })
}

fn alice_party(amount: i64) -> Value {
    json!({ "discord_id": ALICE_DISCORD_ID.to_string(), "amount": amount.to_string() })
}

fn bob_party(amount: i64) -> Value {
    json!({ "discord_id": BOB_DISCORD_ID.to_string(), "amount": amount.to_string() })
}

async fn create_contract(pool: &PgPool, token: &str, body: Value) -> Value {
    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        Some(token),
        body,
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);

    response.body
}

async fn id_of(pool: &PgPool, token: &str, body: Value) -> i64 {
    let created = create_contract(pool, token, body).await;

    created["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a number")
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

/// An application asks, and the contract names who it is for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_asks_and_the_contract_names_its_parties(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let created = create_contract(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100), bob_party(50)])),
    )
    .await;

    assert_eq!(created["unit"], "nyan");
    assert_eq!(created["guild_id"], GUILD.to_string());
    assert_eq!(created["status"], "pending");
    assert_eq!(created["receiver_discord_id"], Value::Null);
    assert_eq!(created["expires_at"], Value::Null, "a permanent one");
    assert_eq!(created["remaining"], "0", "nothing is locked yet");
    assert_eq!(
        created["parties"],
        json!([
            {
                "discord_id": ALICE_DISCORD_ID.to_string(),
                "amount": "100",
                "remaining": "0",
                "status": "pending",
            },
            {
                "discord_id": BOB_DISCORD_ID.to_string(),
                "amount": "50",
                "remaining": "0",
                "status": "pending",
            },
        ])
    );
}

/// The parties are the ask: a contract with none, one naming someone twice, or
/// one asking for nothing is refused.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_contract_without_parties_is_refused(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let empty = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        Some(&fixture.token),
        asked("nyan", json!([])),
    )
    .await;

    assert_eq!(empty.status, 400, "body: {}", empty.body);
    assert_eq!(empty.body["error_description"], "invalid_parties");

    let twice = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        Some(&fixture.token),
        asked("nyan", json!([alice_party(10), alice_party(20)])),
    )
    .await;

    assert_eq!(twice.status, 400, "body: {}", twice.body);
    assert_eq!(twice.body["error_description"], "invalid_parties");

    let nothing = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        Some(&fixture.token),
        asked("nyan", json!([alice_party(0)])),
    )
    .await;

    assert_eq!(nothing.status, 400, "body: {}", nothing.body);
    assert_eq!(nothing.body["error_description"], "invalid_amount");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_unit_is_refused(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        Some(&fixture.token),
        asked("wan", json!([alice_party(100)])),
    )
    .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(response.body["error_description"], "not_found_currency");
}

/// Approving is locking: the amount leaves the party's balance and becomes what
/// the application may spend, from that moment on.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_locks_the_amount(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100), bob_party(50)])),
    )
    .await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["status"], "pending", "one is still to come");
    assert_eq!(response.body["remaining"], "100", "hers, spendable");
    assert_eq!(response.body["parties"][0]["status"], "approved");
    assert_eq!(response.body["parties"][0]["remaining"], "100");
    assert_eq!(balance(&pool, ALICE, 1).await, 900, "locked out of it");
    assert_eq!(balance(&pool, BOB, 1).await, 200, "and nobody else's");
}

/// Approving twice is not a second decision: nothing is locked twice.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_twice_locks_once(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    for _ in 0..2 {
        let response = request(
            vc_api::router(state(pool.clone(), fake())),
            "POST",
            &format!("/api/v2/contracts/{id}/approval"),
            Some(&alice),
            Value::Null,
        )
        .await;

        assert_eq!(response.status, 200, "body: {}", response.body);
    }

    assert_eq!(balance(&pool, ALICE, 1).await, 900);
    assert_eq!(
        response_of(&pool, &fixture.token, id).await["remaining"],
        "100"
    );
}

/// The last approval is the one that says everyone is in.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_last_approval_makes_it_active(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let bob = user_token(&pool, BOB).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100), bob_party(50)])),
    )
    .await;

    for token in [&alice, &bob] {
        request(
            vc_api::router(state(pool.clone(), fake())),
            "POST",
            &format!("/api/v2/contracts/{id}/approval"),
            Some(token),
            Value::Null,
        )
        .await;
    }

    assert_eq!(status_of(&pool, id).await, "active");
    assert_eq!(balance(&pool, ALICE, 1).await, 900);
    assert_eq!(balance(&pool, BOB, 1).await, 150);
}

/// A party who cannot cover what they would lock is refused, and nothing about
/// the contract changes.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_party_who_cannot_cover_it_is_refused(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let bob = user_token(&pool, BOB).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([bob_party(500)])),
    )
    .await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&bob),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(response.body["error_info"], "not_enough_amount");
    assert_eq!(balance(&pool, BOB, 1).await, 200, "untouched");
    assert_eq!(status_of(&pool, id).await, "pending");
}

/// Being named is what lets a user answer: anyone else is answered as a contract
/// that is not there.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn somebody_who_is_not_a_party_cannot_answer(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let stranger = user_token(&pool, STRANGER).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&stranger),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 404, "body: {}", response.body);
    assert_eq!(balance(&pool, STRANGER, 1).await, 0);
}

/// A refusal is the end of it: the contract can never be what it was written as,
/// so the parties who had already locked their amounts take them back.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_refusal_ends_it_and_returns_what_was_locked(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let bob = user_token(&pool, BOB).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100), bob_party(50)])),
    )
    .await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/refusal"),
        Some(&bob),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["status"], "canceled");
    assert_eq!(response.body["remaining"], "0");
    assert_eq!(balance(&pool, ALICE, 1).await, 1_000, "hers came back");
}

/// A permanent delegation is one a party can take back at any time.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_permanent_contract_can_be_withdrawn_from(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "DELETE",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["status"], "canceled");
    assert_eq!(balance(&pool, ALICE, 1).await, 1_000);
}

/// And a temporary one is not: the period is what the party agreed to.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_temporary_contract_cannot_be_withdrawn_from_while_it_runs(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;

    let mut body = asked("nyan", json!([alice_party(100)]));
    body["expires_in"] = json!(3600);

    let id = id_of(&pool, &fixture.token, body).await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "DELETE",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(response.body["error_info"], "invalid_status");
    assert_eq!(balance(&pool, ALICE, 1).await, 900, "still locked");
}

/// Once the deadline has passed, the delegation is over and the remainder is the
/// party's to take back.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_expired_contract_can_be_withdrawn_from(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;

    let mut body = asked("nyan", json!([alice_party(100)]));
    body["expires_in"] = json!(3600);

    let id = id_of(&pool, &fixture.token, body).await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    // The clock is not something a test can move, so the row is what says the
    // deadline has passed.
    sqlx::query!(
        "UPDATE contracts SET expires_at = now() - interval '1 day' WHERE id = $1",
        id
    )
    .execute(&pool)
    .await
    .expect("age the contract");

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "DELETE",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(balance(&pool, ALICE, 1).await, 1_000);
}

/// The application spends what the parties locked, oldest approval first, and
/// the receiver is paid the way a payment would.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_application_spends_what_was_locked(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let bob = user_token(&pool, BOB).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100), bob_party(50)])),
    )
    .await;

    for token in [&alice, &bob] {
        request(
            vc_api::router(state(pool.clone(), fake())),
            "POST",
            &format!("/api/v2/contracts/{id}/approval"),
            Some(token),
            Value::Null,
        )
        .await;
    }

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/payments"),
        Some(&fixture.token),
        json!({
            "receiver_discord_id": STRANGER_DISCORD_ID.to_string(),
            "amount": "120",
        }),
    )
    .await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(response.body["amount"], "120");
    assert_eq!(response.body["remaining"], "30");
    assert_eq!(response.body["unit"], "nyan");

    let contract = response_of(&pool, &fixture.token, id).await;
    assert_eq!(
        contract["parties"][0]["remaining"], "0",
        "the first approval's went first"
    );
    assert_eq!(contract["parties"][1]["remaining"], "30");

    let paid = vc_core::user::find_by_discord_id(&pool, STRANGER_DISCORD_ID)
        .await
        .expect("a lookup")
        .expect("the receiver");
    assert_eq!(balance(&pool, paid.id, 1).await, 120);

    let histories =
        sqlx::query_scalar!("SELECT count(*) AS \"count!\" FROM currency_payment_histories")
            .fetch_one(&pool)
            .await
            .expect("the ledger");

    // The two approvals' locks, and then one row per party whose remainder the
    // payment drew on.
    assert_eq!(histories, 4, "the locks and the payment's own rows");
}

/// More than what is left is refused, and neither the receiver nor the parties
/// see anything move.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payment_of_more_than_is_left_is_refused(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/payments"),
        Some(&fixture.token),
        json!({
            "receiver_discord_id": STRANGER_DISCORD_ID.to_string(),
            "amount": "101",
        }),
    )
    .await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(response.body["error_info"], "not_enough_amount");
    assert_eq!(
        response_of(&pool, &fixture.token, id).await["remaining"],
        "100"
    );
}

/// A contract that names its receiver pays that one and nobody else.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_fixed_receiver_is_the_only_one_paid(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;

    let mut body = asked("nyan", json!([alice_party(100)]));
    body["receiver_discord_id"] = json!(STRANGER_DISCORD_ID.to_string());

    let id = id_of(&pool, &fixture.token, body).await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    let elsewhere = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/payments"),
        Some(&fixture.token),
        json!({ "receiver_discord_id": BOB_DISCORD_ID.to_string(), "amount": "10" }),
    )
    .await;

    assert_eq!(elsewhere.status, 400, "body: {}", elsewhere.body);
    assert_eq!(elsewhere.body["error_description"], "receiver_is_fixed");

    let to_them = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/payments"),
        Some(&fixture.token),
        json!({ "receiver_discord_id": STRANGER_DISCORD_ID.to_string(), "amount": "10" }),
    )
    .await;

    assert_eq!(to_them.status, 201, "body: {}", to_them.body);
}

/// Nothing may be spent once the deadline has passed.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payment_after_the_deadline_is_refused(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;

    let mut body = asked("nyan", json!([alice_party(100)]));
    body["expires_in"] = json!(3600);

    let id = id_of(&pool, &fixture.token, body).await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    sqlx::query!(
        "UPDATE contracts SET expires_at = now() - interval '1 second' WHERE id = $1",
        id
    )
    .execute(&pool)
    .await
    .expect("age the contract");

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/payments"),
        Some(&fixture.token),
        json!({ "receiver_discord_id": STRANGER_DISCORD_ID.to_string(), "amount": "10" }),
    )
    .await;

    assert_eq!(response.status, 409, "body: {}", response.body);
    assert_eq!(response.body["error_info"], "expired");
}

/// Another application's contract is not this application's.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn another_application_cannot_spend_it(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&alice),
        Value::Null,
    )
    .await;

    let thief = insert_application(&pool, OWNER_DISCORD_ID, "theirs").await;
    let account = account_of(&pool, thief).await;
    let thief_token = mint_app(&pool, account, &["vc.contract"]).await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/payments"),
        Some(&thief_token),
        json!({ "receiver_discord_id": STRANGER_DISCORD_ID.to_string(), "amount": "10" }),
    )
    .await;

    assert_eq!(response.status, 404, "body: {}", response.body);
}

/// The application that wrote a contract and the people it names can read it; a
/// stranger is answered as one that is not there.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn only_its_own_people_can_read_it(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let stranger = user_token(&pool, STRANGER).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    for token in [&fixture.token, &alice] {
        let response = request(
            vc_api::router(state(pool.clone(), fake())),
            "GET",
            &format!("/api/v2/contracts/{id}"),
            Some(token),
            Value::Null,
        )
        .await;

        assert_eq!(response.status, 200, "body: {}", response.body);
    }

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        &format!("/api/v2/contracts/{id}"),
        Some(&stranger),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 404, "body: {}", response.body);
}

/// The user's own list is the contracts they are named in, and only those.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_users_list_names_the_contracts_they_are_in(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = user_token(&pool, ALICE).await;
    let stranger = user_token(&pool, STRANGER).await;

    id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100), bob_party(50)])),
    )
    .await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        "/api/v2/users/@me/contracts",
        Some(&alice),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body.as_array().map(Vec::len), Some(1));

    let none = request(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        "/api/v2/users/@me/contracts",
        Some(&stranger),
        Value::Null,
    )
    .await;

    assert_eq!(none.body, json!([]));
}

/// An application's token is answered an empty list rather than a refusal: `@me`
/// is whatever the token is, and a contract names people — an application's
/// account has no Discord id, so it is named in nothing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_applications_list_is_empty(pool: PgPool) {
    let fixture = fixture(&pool).await;

    id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    let response = request(
        vc_api::router(state(pool, fake())),
        "GET",
        "/api/v2/users/@me/contracts",
        Some(&fixture.token),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, json!([]));
}

/// The scope says an application may ask; a token without it cannot, and a user
/// token is not an application at all.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn asking_needs_the_scope_and_the_kind(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let account = account_of(&pool, fixture.application).await;
    let unscoped = mint_app(&pool, account, &["vc.pay"]).await;

    let refused = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        Some(&unscoped),
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    assert_eq!(refused.status, 403, "body: {}", refused.body);
    assert_eq!(refused.body["error"], "insufficient_scope");

    let alice = user_token(&pool, ALICE).await;

    let wrong_kind = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        Some(&alice),
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    assert_eq!(wrong_kind.status, 403, "body: {}", wrong_kind.body);
    assert_eq!(wrong_kind.body["error"], "invalid_token");
}

/// The receiver and the amount are strings like every number here, and one that
/// is not a number is not a payment.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payment_body_is_read_as_strings(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([alice_party(100)])),
    )
    .await;

    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/payments"),
        Some(&fixture.token),
        json!({ "receiver_discord_id": STRANGER_DISCORD_ID.to_string(), "amount": "ten" }),
    )
    .await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        response.body["error_description"],
        "invalid_format_of_amount"
    );
}

/// The contract as its own endpoint answers it, which is what the assertions
/// about state read.
async fn response_of(pool: &PgPool, token: &str, id: i64) -> Value {
    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        &format!("/api/v2/contracts/{id}"),
        Some(token),
        Value::Null,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    response.body
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn unsorted_parties_keep_their_own_amounts(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let id = id_of(
        &pool,
        &fixture.token,
        asked("nyan", json!([bob_party(50), alice_party(100)])),
    )
    .await;
    let token = user_token(&pool, ALICE).await;
    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert!(response.status < 300, "{}", response.body);
    assert_eq!(
        balance(&pool, ALICE, 1).await,
        900,
        "Alice agreed to 100, not Bob's 50"
    );
    let token = user_token(&pool, BOB).await;
    let response = request(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert!(response.status < 300, "{}", response.body);
    assert_eq!(balance(&pool, BOB, 1).await, 150);
}
