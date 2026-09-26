//! Invalid targets must be refused before money, accounts or idempotency keys change.
mod support;

use axum::{Router, body::Body, http::Request};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::*;
use tower::ServiceExt;

async fn post(app: &Router, token: &str, path: &str, body: Value) -> u16 {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("idempotency-key", "\"target-validation\"")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn snapshot(pool: &PgPool) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
        'assets', (SELECT jsonb_agg(a ORDER BY id) FROM assets a),
        'pools', (SELECT jsonb_agg(c ORDER BY id) FROM currencies c),
        'users', (SELECT count(*) FROM users),
        'claims', (SELECT count(*) FROM claims),
        'contracts', (SELECT count(*) FROM contracts),
        'parties', (SELECT jsonb_agg(p ORDER BY id) FROM contract_parties p),
        'payments', (SELECT count(*) FROM currency_payment_histories),
        'issues', (SELECT count(*) FROM currency_given_histories),
        'keys', (SELECT count(*) FROM payments_idempotency))",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn invalid_ids_never_move_money_or_create_accounts(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "target validation").await;
    let user_token = mint(&pool, 1, &["vc.pay", "vc.claim"]).await;
    let app_token = mint_app(
        &pool,
        account_of(&pool, application).await,
        &["vc.contract"],
    )
    .await;
    let guild_token = insert_grant(&pool, application, money.guild, &["vc.issue"]).await;
    let now = time::OffsetDateTime::now_utc();
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
    let charge_path = format!("/api/v2/contracts/{contract}/payments");
    let app = vc_api::router(state(pool.clone(), fake()));
    let before = snapshot(&pool).await;

    for invalid in ["-1", "0", "+1", "9223372036854775808"] {
        let payment = json!({"unit":"n", "receiver_discord_id":invalid, "amount":"10"});
        for (token, path, body) in [
            (
                &user_token,
                "/api/v2/users/@me/transactions",
                payment.clone(),
            ),
            (
                &user_token,
                "/api/v2/users/@me/transactions",
                json!([
                {"unit":"n", "receiver_discord_id":MONEY_USER2.to_string(), "amount":"1"}, payment]),
            ),
            (
                &guild_token,
                "/api/v2/currencies/issue",
                json!({"receiver_discord_id":invalid,"amount":"10"}),
            ),
            (
                &user_token,
                "/api/v2/users/@me/claims",
                json!({"unit":"n","payer_discord_id":invalid,"amount":"10"}),
            ),
            (
                &app_token,
                "/api/v2/contracts",
                json!({"unit":"n","receiver_discord_id":invalid,
                "parties":[{"discord_id":MONEY_USER1.to_string(),"amount":"10"}]}),
            ),
            (
                &app_token,
                "/api/v2/contracts",
                json!({"unit":"n","parties":[{"discord_id":invalid,"amount":"10"}]}),
            ),
            (
                &app_token,
                charge_path.as_str(),
                json!({"receiver_discord_id":invalid,"amount":"10"}),
            ),
            (
                &app_token,
                charge_path.as_str(),
                json!({"receiver_discord_id":MONEY_USER2.to_string(),"party_discord_id":invalid,"amount":"10"}),
            ),
        ] {
            assert_eq!(
                post(&app, token, path, body).await,
                400,
                "{path}: {invalid}"
            );
            assert_eq!(snapshot(&pool).await, before, "{path}: {invalid}");
        }
    }
    // No invalid request claimed the key, and a valid ID above 2^53 stays exact.
    assert_eq!(
        post(
            &app,
            &user_token,
            "/api/v2/users/@me/transactions",
            json!({"unit":"n","receiver_discord_id":"9007199254740993","amount":"10"})
        )
        .await,
        201
    );
    assert_eq!(
        get_amount(&pool, 9007199254740993, money.currency).await,
        10
    );
}
