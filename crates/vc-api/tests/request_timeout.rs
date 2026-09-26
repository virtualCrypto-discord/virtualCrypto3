//! Requests may finish after revocation within the ordinary timeout, but a
//! blocked write must not survive it and commit after its locks become free.

mod support;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    io::{Read, Write},
    time::Duration,
};
use support::*;
use time::OffsetDateTime;
use tower::ServiceExt;

async fn fixture(pool: &PgPool) -> (String, i64) {
    setup_money(pool).await;
    let application = insert_application(pool, MONEY_USER1, "request timeout").await;
    let token = insert_personal_grant(
        pool,
        application,
        MONEY_USER1,
        &["vc.delegate.payments.create"],
        &[1],
    )
    .await;
    let grant =
        vc_core::grant::resolve_token(pool, token.parse().unwrap(), OffsetDateTime::now_utc())
            .await
            .unwrap()
            .unwrap();
    (token, grant.grant_id)
}

async fn runtime_pool(pool: &PgPool) -> PgPool {
    vc_core::db::pool_options(2)
        .connect_with(sqlx::postgres::PgConnectOptions::clone(
            &pool.connect_options(),
        ))
        .await
        .unwrap()
}

fn payment(token: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v2/users/@me/transactions")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("idempotency-key", "\"bounded-payment\"")
        .body(Body::from(
            json!({"unit":"n","amount":"10","receiver_discord_id":MONEY_USER2.to_string()})
                .to_string(),
        ))
        .unwrap()
}

async fn wait_for_lock(pool: &PgPool, query: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)")
                .bind(format!("{query}%")).fetch_one(pool).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("request reached the blocked write");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_request_already_authorized_can_finish_within_the_limit(pool: PgPool) {
    let (token, grant_id) = fixture(&pool).await;
    let api_pool = runtime_pool(&pool).await;
    let app = vc_api::router(state(api_pool.clone(), fake()));
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = 1 FOR NO KEY UPDATE")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pending = tokio::spawn(app.clone().oneshot(payment(&token)));
    wait_for_lock(&pool, "SELECT id FROM users WHERE id = ANY").await;
    assert!(
        vc_core::grant::revoke_one(&pool, grant_id, MONEY_USER1, None)
            .await
            .unwrap()
    );
    assert_eq!(app.oneshot(payment(&token)).await.unwrap().status(), 401);
    blocker.rollback().await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(get_amount(&pool, MONEY_USER2, 1).await, 1010);
    api_pool.close().await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_timed_out_write_rolls_back_balances_history_and_idempotency(pool: PgPool) {
    let (token, grant_id) = fixture(&pool).await;
    let api_pool = runtime_pool(&pool).await;
    let app = vc_api::router(state(api_pool.clone(), fake()));
    let mut blocker = pool.begin().await.unwrap();
    // Transfer has already changed both balances when it reaches this insert.
    sqlx::query("LOCK TABLE currency_payment_histories IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pending = tokio::spawn(app.oneshot(payment(&token)));
    wait_for_lock(&pool, "INSERT INTO currency_payment_histories").await;
    assert!(
        vc_core::grant::revoke_one(&pool, grant_id, MONEY_USER1, None)
            .await
            .unwrap()
    );
    let response =
        tokio::time::timeout(vc_core::db::QUERY_TIMEOUT + Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(response.status(), 504);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body, json!({"error":"request_timeout"}));
    // While the gate is still held, the server-side statement timeout must
    // release the outstanding query/transaction and allow the pool to close.
    tokio::time::timeout(Duration::from_secs(5), api_pool.close())
        .await
        .expect("the timed-out SQL must also stop");
    blocker.rollback().await.unwrap();
    assert_eq!(get_amount(&pool, MONEY_USER1, 1).await, 199500);
    assert_eq!(get_amount(&pool, MONEY_USER2, 1).await, 1000);
    let histories: i64 = sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM payments_idempotency")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((histories, claims), (0, 0));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unfinished_request_body_cannot_keep_authorization_alive(pool: PgPool) {
    let (token, _) = fixture(&pool).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app: Router = vc_api::router(state(pool.clone(), fake()));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = tokio::task::spawn_blocking(move || {
        let mut socket = std::net::TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(40)))
            .unwrap();
        // Send valid headers but only the first byte of the declared JSON body.
        write!(socket, "POST /api/v2/users/@me/transactions HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{").unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        response
    });
    let response = tokio::time::timeout(Duration::from_secs(40), client)
        .await
        .unwrap()
        .unwrap();
    server.abort();
    assert!(response.starts_with("HTTP/1.1 504"), "{response}");
    assert!(response.contains("request_timeout"));
    assert_eq!(get_amount(&pool, MONEY_USER1, 1).await, 199500);
    assert_eq!(get_amount(&pool, MONEY_USER2, 1).await, 1000);
}
