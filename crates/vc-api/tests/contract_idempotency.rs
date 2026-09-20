//! The `Idempotency-Key` a charge may carry: a retry is the same charge, and a
//! refusal is an answer a retry gets rather than a second attempt at.
//!
//! The layer itself is `routes/idempotency.rs`, shared with the transactions and
//! issuing endpoints. What these pin is that a contract payment is wired to it
//! the way they are: the key belongs to the application, the answer is stored
//! whatever it turned out to be, and the money moves once.

mod support;

use axum::http::HeaderValue;
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

/// What the subscriber locks, which is the whole of what may be billed.
const QUOTA: i64 = 100;

const KEY: &str = "a-charge-key";

/// An application that may charge, and a contract of Alice's it has locked: one
/// party, approved, with the receiver fixed to the operator.
struct Fixture {
    token: String,
    contract: i64,
    /// The account the application is, which is what its keys are scoped by.
    account: i32,
}

async fn fixture(pool: &PgPool) -> Fixture {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, ALICE, ALICE_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, ALICE, 1, 1_000).await;

    let application = insert_application(pool, OWNER_DISCORD_ID, "a metered service").await;
    let account = account_of(pool, application).await;
    let token = mint_app(pool, account, &["vc.contract"]).await;
    let contract = created(pool, &token).await;

    Fixture {
        token,
        contract,
        account,
    }
}

/// A contract of Alice's that `token`'s application wrote, approved and ready to
/// charge.
async fn created(pool: &PgPool, token: &str) -> i64 {
    let created = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        "/api/v2/contracts",
        token,
        None,
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

    let contract: i64 = created.body["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a number");

    let alice = mint(pool, ALICE, &[]).await;

    let approved = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{contract}/approval"),
        &alice,
        None,
        Value::Null,
    )
    .await;

    assert_eq!(approved.status, 200, "body: {}", approved.body);

    contract
}

async fn send(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    key: Option<HeaderValue>,
    body: Value,
) -> Response {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json")
        .header("authorization", format!("Bearer {token}"));

    if !body.is_null() {
        builder = builder.header("content-type", "application/json");
    }

    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
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

fn key_header(key: &str) -> HeaderValue {
    HeaderValue::from_bytes(format!("\"{key}\"").as_bytes()).expect("a valid header value")
}

/// The header value verbatim, for keys that are deliberately malformed.
fn raw_header(value: &str) -> HeaderValue {
    HeaderValue::from_bytes(value.as_bytes()).expect("a valid header value")
}

fn idempotency_status(response: &Response) -> Option<String> {
    response
        .headers
        .get("idempotency-status")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

async fn charge(
    pool: &PgPool,
    fixture: &Fixture,
    amount: i64,
    key: Option<HeaderValue>,
) -> Response {
    send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        &fixture.token,
        key,
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": amount.to_string(),
        }),
    )
    .await
}

async fn remaining(pool: &PgPool, fixture: &Fixture) -> Value {
    let response = send(
        vc_api::router(state(pool.clone(), fake())),
        "GET",
        &format!("/api/v2/contracts/{}", fixture.contract),
        &fixture.token,
        None,
        Value::Null,
    )
    .await;

    response.body["remaining"].clone()
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

/// The charge that is retried is one charge: the second request gets the first
/// request's answer, and nothing moves twice.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_repeated_key_charges_once(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let first = charge(&pool, &fixture, 25, Some(key_header(KEY))).await;

    assert_eq!(first.status, 201, "body: {}", first.body);
    assert_eq!(idempotency_status(&first).as_deref(), Some("OK"));
    assert_eq!(first.body["remaining"], "75");

    let retried = charge(&pool, &fixture, 25, Some(key_header(KEY))).await;

    assert_eq!(retried.status, 201, "body: {}", retried.body);
    assert_eq!(idempotency_status(&retried).as_deref(), Some("Duplicate"));
    assert_eq!(retried.body, first.body, "the first answer, verbatim");
    assert_eq!(balance(&pool, ALICE).await, 900, "locked once, not twice");
    assert_eq!(remaining(&pool, &fixture).await, "75");
}

/// A refusal is an answer too. A retry of a charge that was refused gets the
/// refusal it got rather than a second attempt at spending money that is gone.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_refusal_is_cached_too(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let refused = charge(&pool, &fixture, 200, Some(key_header(KEY))).await;

    assert_eq!(refused.status, 409, "body: {}", refused.body);
    assert_eq!(refused.body["error_info"], "not_enough_amount");
    assert_eq!(idempotency_status(&refused).as_deref(), Some("OK"));

    let retried = charge(&pool, &fixture, 200, Some(key_header(KEY))).await;

    assert_eq!(retried.status, 409, "body: {}", retried.body);
    assert_eq!(retried.body, refused.body);
    assert_eq!(idempotency_status(&retried).as_deref(), Some("Duplicate"));

    // And the key is what was cached, not the endpoint: another key still spends.
    let other = charge(&pool, &fixture, 25, Some(key_header("another-key"))).await;

    assert_eq!(other.status, 201, "body: {}", other.body);
    assert_eq!(other.body["remaining"], "75");
}

/// The key is the application's, so a token that could not charge without one
/// cannot charge with one either — and nothing is cached for it to collect.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_key_does_not_make_a_scopeless_token_able_to_charge(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let application = insert_application(&pool, OWNER_DISCORD_ID, "another").await;
    let account = account_of(&pool, application).await;
    let unscoped = mint_app(&pool, account, &["vc.pay"]).await;

    let response = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        &unscoped,
        Some(key_header(KEY)),
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": "10",
        }),
    )
    .await;

    assert_eq!(response.status, 403, "body: {}", response.body);
    assert_eq!(response.body["error"], "insufficient_scope");
    assert_eq!(idempotency_status(&response), None);
}

/// A request that never became a charge does not spend the key: the body it
/// could not read is fixed and sent again under the same one, and that request is
/// the charge rather than a replay of the typo.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_body_that_does_not_parse_does_not_spend_the_key(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let malformed = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        &fixture.token,
        Some(key_header(KEY)),
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": "ten",
        }),
    )
    .await;

    assert_eq!(malformed.status, 400, "body: {}", malformed.body);
    assert_eq!(
        malformed.body["error_description"],
        "invalid_format_of_amount"
    );
    assert_eq!(idempotency_status(&malformed), None, "nothing was claimed");
    assert_eq!(
        claimed(&pool, &fixture, KEY).await,
        0,
        "and no row was left behind"
    );

    let corrected = charge(&pool, &fixture, 25, Some(key_header(KEY))).await;

    assert_eq!(corrected.status, 201, "body: {}", corrected.body);
    assert_eq!(idempotency_status(&corrected).as_deref(), Some("OK"));
    assert_eq!(corrected.body["remaining"], "75");
    assert_eq!(claimed(&pool, &fixture, KEY).await, 1, "now it is claimed");
}

/// And neither does a caller who is not an application: being refused for what
/// they are must not spend it either.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_that_is_not_an_applications_does_not_spend_the_key(pool: PgPool) {
    let fixture = fixture(&pool).await;
    let alice = mint(&pool, ALICE, &[]).await;

    let refused = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        &alice,
        Some(key_header(KEY)),
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": "25",
        }),
    )
    .await;

    assert_eq!(refused.status, 403, "body: {}", refused.body);
    assert_eq!(
        claimed_by(&pool, ALICE, KEY).await,
        0,
        "the key the caller would have used is untouched"
    );

    let charged = charge(&pool, &fixture, 25, Some(key_header(KEY))).await;

    assert_eq!(charged.status, 201, "body: {}", charged.body);
    assert_eq!(charged.body["remaining"], "75");
}

/// The rows a key has in the layer's own table, which is what says whether a
/// request claimed it: one that answered before claiming left none.
async fn claimed(pool: &PgPool, fixture: &Fixture, key: &str) -> i64 {
    claimed_by(pool, fixture.account, key).await
}

async fn claimed_by(pool: &PgPool, account: i32, key: &str) -> i64 {
    sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM payments_idempotency
          WHERE idempotency_key = $1 AND user_id = $2",
        key.as_bytes().to_vec(),
        i64::from(account)
    )
    .fetch_one(pool)
    .await
    .expect("the layer's table")
}

/// A key that is not the quoted token the specification asks for is refused
/// before anything is claimed.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unquoted_key_is_rejected(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let response = charge(&pool, &fixture, 10, Some(raw_header(KEY))).await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        response.body["error_description"],
        "invalid_idempotency_key"
    );
    assert_eq!(idempotency_status(&response), None);
    assert_eq!(remaining(&pool, &fixture).await, QUOTA.to_string());
}

/// Without a key there is no layer, and the answer says which of the three
/// things happened.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_charge_without_a_key_says_so(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let response = charge(&pool, &fixture, 25, None).await;

    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(
        idempotency_status(&response).as_deref(),
        Some("Not Requested")
    );
}

/// A failure is not the layer's to dress: what carried no key is answered exactly
/// as it would have been without the layer, while a *refusal* — an answer the
/// endpoint produced — does carry the header that says so.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_failure_without_a_key_carries_no_layer_header(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let malformed = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", fixture.contract),
        &fixture.token,
        None,
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": "ten",
        }),
    )
    .await;

    assert_eq!(malformed.status, 400, "body: {}", malformed.body);
    assert_eq!(
        idempotency_status(&malformed),
        None,
        "not the layer's answer"
    );

    let refused = charge(&pool, &fixture, 200, None).await;

    assert_eq!(refused.status, 409, "body: {}", refused.body);
    assert_eq!(
        idempotency_status(&refused).as_deref(),
        Some("Not Requested"),
        "an answer, from an endpoint that was not asked to remember it"
    );
}

/// The key is the application's, so the same one used by another application is
/// another key — and both charges happen.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn another_application_may_use_the_same_key(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let mine = charge(&pool, &fixture, 25, Some(key_header(KEY))).await;

    assert_eq!(mine.status, 201, "body: {}", mine.body);
    assert_eq!(idempotency_status(&mine).as_deref(), Some("OK"));

    let theirs = second_application(&pool).await;
    let charged = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{}/payments", theirs.contract),
        &theirs.token,
        Some(key_header(KEY)),
        json!({
            "receiver_discord_id": RECEIVER_DISCORD_ID.to_string(),
            "amount": "25",
        }),
    )
    .await;

    assert_eq!(charged.status, 201, "body: {}", charged.body);
    assert_eq!(
        idempotency_status(&charged).as_deref(),
        Some("OK"),
        "their key, not mine"
    );

    assert_eq!(balance(&pool, ALICE).await, 800, "both approvals locked");
}

/// Another application, with a contract of its own, which Alice also approved.
async fn second_application(pool: &PgPool) -> Fixture {
    let application = insert_application(pool, OWNER_DISCORD_ID, "another service").await;
    let account = account_of(pool, application).await;
    let token = mint_app(pool, account, &["vc.contract"]).await;

    let contract = created(pool, &token).await;

    let alice = mint(pool, ALICE, &[]).await;

    let approved = send(
        vc_api::router(state(pool.clone(), fake())),
        "POST",
        &format!("/api/v2/contracts/{contract}/approval"),
        &alice,
        None,
        Value::Null,
    )
    .await;

    assert_eq!(approved.status, 200, "body: {}", approved.body);

    Fixture {
        token,
        contract,
        account,
    }
}

/// A failure is not an answer: the charge's transaction rolled back, nothing
/// happened, and the key goes back — so the caller can attempt the same operation
/// again with the key it already chose, rather than being told for a week what
/// went wrong once.
///
/// The table the charge reads is dropped rather than the failure simulated: this
/// is the real path, and it is also what says the retry re-attempts instead of
/// reading a stored error back.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_failure_gives_the_key_back(pool: PgPool) {
    let fixture = fixture(&pool).await;

    sqlx::query("DROP TABLE contract_parties CASCADE")
        .execute(&pool)
        .await
        .expect("the tables a charge reads");

    let failed = charge(&pool, &fixture, 25, Some(key_header(KEY))).await;

    assert_eq!(failed.status, 500, "body: {}", failed.body);
    assert_eq!(idempotency_status(&failed), None, "not the layer's answer");
    assert_eq!(
        claimed(&pool, &fixture, KEY).await,
        0,
        "and the key went back"
    );

    let retried = charge(&pool, &fixture, 25, Some(key_header(KEY))).await;

    assert_eq!(retried.status, 500, "body: {}", retried.body);
    assert_eq!(
        idempotency_status(&retried),
        None,
        "attempted again rather than read back — a replay would carry Duplicate"
    );
    assert_eq!(
        escrow(&pool, fixture.contract).await,
        QUOTA,
        "and the charges that failed moved nothing"
    );
}

/// What the contract's own account holds, which is the locked quota until
/// something is charged against it.
async fn escrow(pool: &PgPool, contract: i64) -> i64 {
    sqlx::query_scalar!(
        "SELECT COALESCE(SUM(a.amount), 0)::bigint AS \"total!\"
           FROM assets a
           JOIN users u ON u.id = a.user_id
          WHERE u.contract_id = $1",
        contract
    )
    .fetch_one(pool)
    .await
    .expect("the escrow")
}

/// Two requests with one key, at the same time: the second waits on the first
/// rather than racing it, and what it is answered with is the first one's answer.
/// One charge, one answer — whichever order they happen to finish in.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn concurrent_charges_with_one_key_write_once(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let (first, second) = tokio::join!(
        charge(&pool, &fixture, 25, Some(key_header(KEY))),
        charge(&pool, &fixture, 25, Some(key_header(KEY))),
    );

    assert_eq!(first.status, 201, "body: {}", first.body);
    assert_eq!(second.status, 201, "body: {}", second.body);
    assert_eq!(first.body, second.body, "one answer for both of them");

    let mut statuses = [
        idempotency_status(&first).unwrap_or_default(),
        idempotency_status(&second).unwrap_or_default(),
    ];
    statuses.sort();
    assert_eq!(
        statuses,
        ["Duplicate".to_owned(), "OK".to_owned()],
        "one did the work, and the other read it back"
    );

    assert_eq!(remaining(&pool, &fixture).await, "75", "charged once");
}

/// A request whose key another one is holding waits — and stops waiting. The
/// holder here is a transaction the test never commits, so the claim blocks for
/// exactly as long as the layer allows and the answer is the one that tells a
/// client to come back, rather than a request sitting on the row.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_key_another_request_is_holding_answers_come_back(pool: PgPool) {
    let fixture = fixture(&pool).await;

    let mut holder = pool.begin().await.expect("a transaction");
    sqlx::query!(
        "INSERT INTO payments_idempotency
             (user_id, idempotency_key, expires, inserted_at, updated_at)
         VALUES ($1, $2, now() + interval '7 days', now(), now())",
        i64::from(fixture.account),
        KEY.as_bytes().to_vec()
    )
    .execute(&mut *holder)
    .await
    .expect("a claim nobody finishes");

    let refused = charge(&pool, &fixture, 25, Some(key_header(KEY))).await;

    assert_eq!(refused.status, 409, "body: {}", refused.body);
    assert_eq!(refused.body["error"], "processing");
    assert_eq!(idempotency_status(&refused).as_deref(), Some("Duplicate"));
    assert_eq!(
        remaining(&pool, &fixture).await,
        QUOTA.to_string(),
        "and nothing was charged"
    );

    holder.rollback().await.expect("the claim goes away");

    let charged = charge(&pool, &fixture, 25, Some(key_header(KEY))).await;

    assert_eq!(charged.status, 201, "body: {}", charged.body);
    assert_eq!(charged.body["remaining"], "75");
}
