//! Cached answers require the operation and delegation that wrote them, even
//! when two credentials act on the same account and carry the same key.

mod support;

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::*;
use time::OffsetDateTime;
use tower::ServiceExt;

const KEY: &str = "shared-invoice";
const PAYMENTS: &str = "/api/v2/users/@me/transactions";
const ISSUE: &str = "/api/v2/currencies/issue";

async fn post(pool: &PgPool, uri: &str, token: &str, body: Value) -> Response {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("idempotency-key", format!("\"{KEY}\""))
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Response {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap(),
    }
}

fn payment() -> Value {
    json!({"receiver_discord_id": MONEY_USER2.to_string(), "unit": "n", "amount": "10"})
}

fn assert_conflict(response: &Response) {
    assert_eq!(response.status, 409, "{}", response.body);
    assert_eq!(
        response.body,
        json!({"error": "conflict", "error_description": "idempotency_context_mismatch"})
    );
    assert_eq!(response.headers["idempotency-status"], "Duplicate");
    assert!(!response.headers.contains_key("retry-after"));
}

fn assert_replay(response: &Response, original: &Response) {
    assert_eq!(response.status, original.status);
    assert_eq!(response.body, original.body);
    assert_eq!(response.headers["idempotency-status"], "Duplicate");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn guild_token_cannot_replay_a_private_contract_payment(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "contract owner").await;
    let account = account_of(&pool, application).await;
    let app_token = mint_app(&pool, account, &["vc.contract", "vc.pay"]).await;
    let guild_token = insert_grant_for(
        &pool,
        application,
        money.guild2,
        &["vc.issue"],
        &[money.currency2],
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let contract = vc_core::contract::create(
        &pool,
        application,
        "n",
        &[vc_core::contract::NewParty {
            discord_id: MONEY_USER1,
            amount: 100,
        }],
        None,
        None,
        now,
    )
    .await
    .unwrap();
    vc_core::contract::approve(&pool, contract, 1, || now)
        .await
        .unwrap();
    let uri = format!("/api/v2/contracts/{contract}/payments");
    let original = post(&pool, &uri, &app_token, payment()).await;
    assert_eq!(original.status, 201, "{}", original.body);
    assert_eq!(original.body["remaining"], "90");

    assert_conflict(&post(&pool, ISSUE, &guild_token, payment()).await);
    // The app's own wallet endpoint also needs its own operation's authority.
    assert_conflict(&post(&pool, PAYMENTS, &app_token, payment()).await);
    assert_replay(&post(&pool, &uri, &app_token, payment()).await, &original);
    assert_eq!(get_amount(&pool, MONEY_USER2, money.currency).await, 1010);
    assert_eq!(
        get_amount(&pool, MONEY_USER2, money.currency2).await,
        200000
    );
    assert_eq!(
        currency_by_unit(&pool, &money.unit2)
            .await
            .unwrap()
            .pool_amount,
        Some(1000)
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn different_guilds_cannot_share_an_applications_cached_issue(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "issuer").await;
    let first = insert_grant_for(
        &pool,
        application,
        money.guild,
        &["vc.issue"],
        &[money.currency],
    )
    .await;
    let second = insert_grant_for(
        &pool,
        application,
        money.guild2,
        &["vc.issue"],
        &[money.currency2],
    )
    .await;
    let original = post(&pool, ISSUE, &first, payment()).await;
    assert_eq!(original.status, 201);
    assert_eq!(original.body["unit"], "n");
    assert_conflict(&post(&pool, ISSUE, &second, payment()).await);
    assert_replay(&post(&pool, ISSUE, &first, payment()).await, &original);
    assert_eq!(get_amount(&pool, MONEY_USER2, money.currency).await, 1010);
    assert_eq!(
        get_amount(&pool, MONEY_USER2, money.currency2).await,
        200000
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn personal_grants_and_own_credentials_cannot_replay_each_others_payments(pool: PgPool) {
    let money = setup_money(&pool).await;
    let first_app = insert_application(&pool, MONEY_USER1, "first payer").await;
    let second_app = insert_application(&pool, MONEY_USER1, "second payer").await;
    let first = insert_personal_grant(
        &pool,
        first_app,
        MONEY_USER1,
        &["vc.delegate.payments.create"],
        &[money.currency],
    )
    .await;
    let second = insert_personal_grant(
        &pool,
        second_app,
        MONEY_USER1,
        &["vc.delegate.payments.create"],
        &[money.currency],
    )
    .await;
    let own = mint(&pool, 1, &["vc.pay"]).await;
    let original = post(&pool, PAYMENTS, &first, payment()).await;
    assert_eq!(original.status, 201);
    for token in [&second, &own] {
        assert_conflict(&post(&pool, PAYMENTS, token, payment()).await);
    }
    assert_replay(&post(&pool, PAYMENTS, &first, payment()).await, &original);
    assert_eq!(get_amount(&pool, MONEY_USER1, money.currency).await, 199490);
    assert_eq!(get_amount(&pool, MONEY_USER2, money.currency).await, 1010);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn refreshed_grant_token_replays_without_issuing_twice(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "issuer").await;
    let token = insert_grant_for(
        &pool,
        application,
        money.guild,
        &["vc.issue"],
        &[money.currency],
    )
    .await;
    let original = post(&pool, ISSUE, &token, payment()).await;
    assert_eq!(original.status, 201);
    let now = OffsetDateTime::now_utc();
    let grant = vc_core::grant::resolve_token(&pool, token.parse().unwrap(), now)
        .await
        .unwrap()
        .unwrap();
    let refresh = vc_core::grant::create_refresh_token(&pool, grant.grant_id, now)
        .await
        .unwrap();
    let refreshed = vc_core::grant::exchange_refresh_token(&pool, &refresh, || now)
        .await
        .unwrap();
    assert_ne!(token, refreshed.access_token);
    assert_replay(
        &post(&pool, ISSUE, &refreshed.access_token, payment()).await,
        &original,
    );
    assert_eq!(get_amount(&pool, MONEY_USER2, money.currency).await, 1010);
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .unwrap()
            .pool_amount,
        Some(490)
    );
}

async fn forget_legacy_context(pool: &PgPool) {
    sqlx::query("UPDATE payments_idempotency SET replay_context = NULL")
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn legacy_wallet_success_replays_but_is_not_available_to_a_delegation(pool: PgPool) {
    let money = setup_money(&pool).await;
    let token = mint(&pool, 1, &["vc.pay"]).await;
    let original = post(&pool, PAYMENTS, &token, payment()).await;
    assert_eq!(original.status, 201);
    forget_legacy_context(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "payer").await;
    let delegated = insert_personal_grant(
        &pool,
        application,
        MONEY_USER1,
        &["vc.delegate.payments.create"],
        &[money.currency],
    )
    .await;
    assert_conflict(&post(&pool, PAYMENTS, &delegated, payment()).await);
    assert_replay(&post(&pool, PAYMENTS, &token, payment()).await, &original);
    assert_eq!(get_amount(&pool, MONEY_USER2, money.currency).await, 1010);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn legacy_issue_keeps_its_key_without_disclosing_or_reexecuting_it(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "issuer").await;
    let token = insert_grant_for(
        &pool,
        application,
        money.guild,
        &["vc.issue"],
        &[money.currency],
    )
    .await;
    let original = post(&pool, ISSUE, &token, payment()).await;
    assert_eq!(original.status, 201);
    forget_legacy_context(&pool).await;
    for _ in 0..2 {
        assert_conflict(&post(&pool, ISSUE, &token, payment()).await);
    }
    let cached: Value =
        sqlx::query_scalar("SELECT body FROM payments_idempotency WHERE idempotency_key=$1")
            .bind(KEY.as_bytes())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cached, original.body);
    assert_eq!(get_amount(&pool, MONEY_USER2, money.currency).await, 1010);
    assert_eq!(
        currency_by_unit(&pool, &money.unit)
            .await
            .unwrap()
            .pool_amount,
        Some(490)
    );
}
