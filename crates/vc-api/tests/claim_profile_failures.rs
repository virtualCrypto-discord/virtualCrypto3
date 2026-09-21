mod support;

use axum::{Router, body::Body, http::Request};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;

async fn request(
    router: Router,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let response = router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn claim_lifecycle(pool: PgPool, discord_status: u16) {
    let money = support::setup_money(&pool).await;
    let claimant = support::mint(&pool, 1, &["vc.claim"]).await;
    let payer = support::mint(&pool, 2, &["vc.claim"]).await;
    let router = vc_api::router(support::state(
        pool.clone(),
        Arc::new(support::FakeDiscord::with_user_status(discord_status)),
    ));
    let (status, created) = request(
        router.clone(),
        &claimant,
        "POST",
        "/api/v2/users/@me/claims",
        json!({"unit":money.unit, "payer_discord_id":money.user2.to_string(), "amount":"100"}),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(
        created["claimant"]["discord"],
        json!({"id":money.user1.to_string()})
    );
    assert_eq!(
        created["payer"]["discord"],
        json!({"id":money.user2.to_string()})
    );
    assert_eq!(created["status"], "pending");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM claims")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let path = format!(
        "/api/v2/users/@me/claims/{}",
        created["id"].as_str().unwrap()
    );
    let (status, updated) = request(
        router.clone(),
        &claimant,
        "PATCH",
        &path,
        json!({"metadata":{"note":"kept"}}),
    )
    .await;
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["metadata"], json!({"note":"kept"}));
    let before_payer = support::get_amount(&pool, money.user2, money.currency).await;
    let before_claimant = support::get_amount(&pool, money.user1, money.currency).await;
    let (status, approved) = request(
        router.clone(),
        &payer,
        "PATCH",
        &path,
        json!({"status":"approved"}),
    )
    .await;
    assert_eq!(status, 200, "{approved}");
    assert_eq!(approved["status"], "approved");
    assert_eq!(
        support::get_amount(&pool, money.user2, money.currency).await,
        before_payer - 100
    );
    assert_eq!(
        support::get_amount(&pool, money.user1, money.currency).await,
        before_claimant + 100
    );
    let (status, read) = request(router.clone(), &payer, "GET", &path, Value::Null).await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read, approved);
    let (status, list) = request(
        router.clone(),
        &payer,
        "GET",
        "/api/v2/users/@me/claims?statuses[]=approved",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list, json!([approved]));
    let (status, _) = request(router, &payer, "PATCH", &path, json!({"status":"approved"})).await;
    assert_eq!(status, 409);
    assert_eq!(
        support::get_amount(&pool, money.user2, money.currency).await,
        before_payer - 100
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn discord_outage_does_not_fail_committed_claim_operations(pool: PgPool) {
    claim_lifecycle(pool, 503).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn missing_discord_profiles_do_not_fail_claim_operations(pool: PgPool) {
    claim_lifecycle(pool, 404).await;
}
