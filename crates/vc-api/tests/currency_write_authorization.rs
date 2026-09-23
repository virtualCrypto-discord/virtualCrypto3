//! Currency replacement must not widen a resource-restricted grant, including
//! while an API request waits for another attempt's idempotency claim.

mod support;

use axum::{body::Body, http::Request};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::*;
use tower::ServiceExt;

async fn wait_for_lock(pool: &PgPool, query: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)",
            )
            .bind(format!("{query}%"))
            .fetch_one(pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("the request reached its database lock");
}

fn request(uri: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("idempotency-key", "\"currency-replacement\"")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn assert_refused(pool: &PgPool, response: axum::response::Response) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"], "insufficient_scope");
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM payments_idempotency")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(claims, 0, "a forbidden operation must not consume the key");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issue_checks_the_currency_it_actually_spends(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "issuer").await;
    let token = insert_grant_for(
        &pool,
        application,
        MONEY_GUILD,
        &["vc.issue"],
        &[money.currency],
    )
    .await;
    let account = account_of(&pool, application).await;
    let app = vc_api::router(state(pool.clone(), fake()));
    let mut blocker = pool.begin().await.unwrap();
    vc_core::idempotency::claim_in(&mut blocker, b"currency-replacement", account)
        .await
        .unwrap();
    let pending = tokio::spawn(app.oneshot(request(
        "/api/v2/currencies/issue",
        &token,
        json!({"receiver_discord_id":MONEY_USER2.to_string(),"amount":"1"}),
    )));
    wait_for_lock(&pool, "INSERT INTO payments_idempotency").await;
    assert_eq!(
        vc_core::currency::delete(&pool, MONEY_GUILD, "delete n")
            .await
            .unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    vc_core::currency::create(&pool, MONEY_GUILD, "replacement", "new", MONEY_USER1, 1000)
        .await
        .unwrap();
    blocker.rollback().await.unwrap();
    assert_refused(&pool, pending.await.unwrap().unwrap()).await;
    let balance: i64 = sqlx::query_scalar("SELECT pool_amount FROM currencies WHERE guild_id=$1")
        .bind(MONEY_GUILD)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance, 5);
    let issued: i64 = sqlx::query_scalar("SELECT count(*) FROM currency_given_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(issued, 0);
}

async fn payment_replacement(pool: PgPool, bulk: bool, initially_missing: bool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "payer").await;
    let token = insert_personal_grant(
        &pool,
        application,
        MONEY_USER1,
        &["vc.delegate.payments.create"],
        &[money.currency],
    )
    .await;
    if initially_missing {
        vc_core::currency::delete(&pool, MONEY_GUILD, "delete n")
            .await
            .unwrap();
    }
    let app = vc_api::router(state(pool.clone(), fake()));
    let mut blocker = pool.begin().await.unwrap();
    vc_core::idempotency::claim_in(&mut blocker, b"currency-replacement", 1)
        .await
        .unwrap();
    let payment = json!({"receiver_discord_id":MONEY_USER2.to_string(),"unit":"n","amount":"100"});
    let body = if bulk {
        json!([payment.clone(), payment])
    } else {
        payment
    };
    let pending =
        tokio::spawn(app.oneshot(request("/api/v2/users/@me/transactions", &token, body)));
    wait_for_lock(&pool, "INSERT INTO payments_idempotency").await;
    if !initially_missing {
        assert_eq!(
            vc_core::currency::delete(&pool, MONEY_GUILD, "delete n")
                .await
                .unwrap(),
            vc_core::currency::DeleteResult::Deleted
        );
    }
    vc_core::currency::create(&pool, MONEY_GUILD, "replacement", "n", MONEY_USER1, 1000)
        .await
        .unwrap();
    blocker.rollback().await.unwrap();
    assert_refused(&pool, pending.await.unwrap().unwrap()).await;
    let balance: i64 = sqlx::query_scalar("SELECT a.amount FROM assets a JOIN currencies c ON c.id=a.currency_id WHERE a.user_id=1 AND c.unit='n'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(balance, 1000);
    let paid: i64 = sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(paid, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn payment_checks_the_currency_it_actually_spends(pool: PgPool) {
    payment_replacement(pool, false, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bulk_payment_checks_the_currency_it_actually_spends(pool: PgPool) {
    payment_replacement(pool, true, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_currency_created_after_preflight_is_not_implicitly_authorized(pool: PgPool) {
    payment_replacement(pool, false, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_authorized_unit_cannot_be_replaced_until_the_write_finishes(pool: PgPool) {
    let money = setup_money(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    assert!(
        vc_api::resource::lock_units_in(&mut tx, &[money.currency], &[money.unit])
            .await
            .unwrap()
    );
    let other = pool.clone();
    let deleting = tokio::spawn(async move {
        vc_core::currency::delete(&other, MONEY_GUILD, "delete n")
            .await
            .unwrap()
    });
    wait_for_lock(&pool, "SELECT id, unit, inserted_at FROM currencies").await;
    assert!(!deleting.is_finished());
    tx.commit().await.unwrap();
    assert_eq!(
        deleting.await.unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn claim_creation_refuses_a_currency_deleted_while_authorization_waits(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "claimant").await;
    let token = insert_personal_grant(
        &pool,
        application,
        MONEY_USER1,
        &["vc.delegate.claims.create"],
        &[money.currency],
    )
    .await;
    let mut replacement = pool.begin().await.unwrap();
    // The fixture has no history or claims. Keep the deletion uncommitted so
    // authorization sees the old row and waits for its lock.
    sqlx::query("DELETE FROM assets WHERE currency_id=$1")
        .bind(money.currency)
        .execute(&mut *replacement)
        .await
        .unwrap();
    sqlx::query("DELETE FROM currencies WHERE id=$1")
        .bind(money.currency)
        .execute(&mut *replacement)
        .await
        .unwrap();
    let app = vc_api::router(state(pool.clone(), fake()));
    let pending = tokio::spawn(app.oneshot(request(
        "/api/v2/users/@me/claims",
        &token,
        json!({"payer_discord_id":MONEY_USER2.to_string(),"unit":"n","amount":"10"}),
    )));
    wait_for_lock(&pool, "SELECT id, unit FROM currencies WHERE unit = ANY").await;
    sqlx::query("INSERT INTO currencies (name, unit, guild_id, pool_amount, inserted_at, updated_at) VALUES ('replacement', 'n', $1, 5, now(), now())")
        .bind(MONEY_GUILD).execute(&mut *replacement).await.unwrap();
    replacement.commit().await.unwrap();
    let response = pending.await.unwrap().unwrap();
    assert_eq!(response.status(), 400);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error_description"], "not_found_currency");
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM claims")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(claims, 0);
}
