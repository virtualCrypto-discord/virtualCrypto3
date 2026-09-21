mod support;

use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn user_and_app_tokens_accept_case_insensitive_bearer(pool: PgPool) {
    support::insert_user(&pool, 1, 100000000000000001).await;
    let application = support::insert_application(&pool, 100000000000000001, "Shop").await;
    let account = support::account_of(&pool, application).await;
    let tokens = [
        support::mint(&pool, 1, &["vc.pay"]).await,
        support::mint_app(&pool, account, &["vc.pay"]).await,
    ];
    let app = vc_api::router(support::state(pool, support::fake()));
    for token in tokens {
        for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
            let request = axum::http::Request::builder()
                .uri("/api/v2/users/@me/balances")
                .header("accept", "application/json")
                .header("authorization", format!("{scheme} {token}"))
                .body(axum::body::Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status().as_u16(), 200, "{scheme}");
        }
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn guild_tokens_accept_case_insensitive_bearer(pool: PgPool) {
    let money = support::setup_money(&pool).await;
    let application = support::insert_application(&pool, money.user1, "Shop").await;
    let token = support::insert_grant(&pool, application, money.guild, &["vc.issue"]).await;
    let app = vc_api::router(support::state(pool.clone(), support::fake()));
    for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v2/currencies/issue")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header("authorization", format!("{scheme} {token}"))
            .body(axum::body::Body::from(
                serde_json::json!({
                    "receiver_discord_id": money.user1.to_string(), "amount": "1"
                })
                .to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), 201, "{scheme}");
    }
}
