mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    account_of, fake, insert_application, insert_asset, insert_currency, insert_user, mint,
    mint_app, state,
};
use tower::ServiceExt;

const OWNER: i64 = 500_000_000_000_000_001;
const PARTY: i64 = 100_000_000_000_000_001;
const GUILD: i64 = 900_000_000_000_000_001;

async fn fixture(pool: &PgPool) -> (axum::Router, i64, String) {
    insert_user(pool, 1, OWNER).await;
    insert_user(pool, 2, PARTY).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, 2, 1, 1_000).await;
    let application = insert_application(pool, OWNER, "numbers").await;
    let token = mint_app(
        pool,
        account_of(pool, application).await,
        &["vc.contract", "oauth2.register"],
    )
    .await;
    (
        vc_api::router(state(pool.clone(), fake())),
        application,
        token,
    )
}

// Keep the original JSON notation on the wire, especially exponent notation.
async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: &str,
) -> (u16, Value) {
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)))
    };
    (status, body)
}

fn contract_body(expiry: &str) -> String {
    format!(
        r#"{{"unit":"nyan","parties":[{{"discord_id":"{PARTY}","amount":"100"}}],"expires_in":{expiry}}}"#
    )
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn contract_integer_notations_keep_the_deadline_and_refund(pool: PgPool) {
    let (app, _, token) = fixture(&pool).await;
    let party_token = mint(&pool, 2, &[]).await;
    for (expiry, seconds) in [
        ("3600", 3600_i64),
        ("3600.0", 3600),
        ("3.6e3", 3600),
        ("1.0", 1),
        ("3.1536e7", 31_536_000),
    ] {
        let (status, created) = request(
            app.clone(),
            "POST",
            "/api/v2/contracts",
            &token,
            &contract_body(expiry),
        )
        .await;
        assert_eq!(status, 201, "{expiry}: {created}");
        assert!(created["expires_at"].is_string(), "{expiry}: {created}");
        let id: i64 = created["id"].as_str().unwrap().parse().unwrap();
        let lifetime: Option<i64> = sqlx::query_scalar("SELECT EXTRACT(EPOCH FROM (expires_at - inserted_at))::bigint FROM contracts WHERE id = $1")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(lifetime, Some(seconds), "{expiry}");

        // The one-second boundary need not be approved before the wall clock ticks.
        if seconds == 3600 {
            let (status, body) = request(
                app.clone(),
                "POST",
                &format!("/api/v2/contracts/{id}/approval"),
                &party_token,
                "null",
            )
            .await;
            assert_eq!(status, 200, "{body}");
            assert!(
                vc_core::contract::settle(
                    &pool,
                    id,
                    time::OffsetDateTime::now_utc() + time::Duration::hours(2)
                )
                .await
                .unwrap()
            );
            let balance: i64 = sqlx::query_scalar(
                "SELECT amount FROM assets WHERE user_id = 2 AND currency_id = 1",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(balance, 1_000, "{expiry}: the locked money is refunded");
        }
    }
    let (status, created) = request(
        app,
        "POST",
        "/api/v2/contracts",
        &token,
        &contract_body("null"),
    )
    .await;
    assert_eq!(status, 201);
    assert!(created["expires_at"].is_null());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn invalid_contract_lifetimes_never_create_permanent_contracts(pool: PgPool) {
    let (app, _, token) = fixture(&pool).await;
    for expiry in [
        "3600.5",
        "0.5",
        "-1.5",
        "0",
        "-1.0",
        "31536001",
        "31536000.5",
        "18446744073709551615",
        "1e100",
        "-1e100",
    ] {
        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/v2/contracts",
            &token,
            &contract_body(expiry),
        )
        .await;
        assert_eq!(status, 400, "{expiry}: {body}");
        assert_eq!(body["error_description"], "invalid_expires_in", "{expiry}");
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contracts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn grant_request_lifetimes_accept_integer_notations_without_defaulting(pool: PgPool) {
    let (app, application, token) = fixture(&pool).await;
    for (index, expiry) in [
        "3600",
        "3600.0",
        "3.6e3",
        "3600.5",
        "-1.5",
        "0.0",
        "3601.0",
        "18446744073709551615",
        "null",
    ]
    .iter()
    .enumerate()
    {
        // Distinct targets prevent reuse of an earlier request masking its lifetime.
        let guild = GUILD + index as i64;
        let body =
            format!(r#"{{"guild_id":"{guild}","scopes":["vc.issue"],"expires_in":{expiry}}}"#);
        let (status, body) = request(
            app.clone(),
            "POST",
            "/oauth2/clients/@me/grant-requests",
            &token,
            &body,
        )
        .await;
        if index < 3 || *expiry == "null" {
            assert_eq!(status, 201, "{expiry}: {body}");
            assert_eq!(
                body["expires_in"],
                if *expiry == "null" { 600 } else { 3600 }
            );
        } else {
            assert!([400, 422].contains(&status), "{expiry}: {status} {body}");
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM grant_requests WHERE application_id = $1 AND guild_id = $2",
            )
            .bind(application)
            .bind(guild)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(count, 0, "{expiry}");
        }
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn event_subscriptions_accept_integer_notations_and_reject_fractions(pool: PgPool) {
    let (app, _, _) = fixture(&pool).await;
    support::insert_discord_auth(&pool, OWNER, "a-discord-token").await;
    let token = mint(&pool, 1, &["oauth2.register"]).await;
    let (status, registered) = request(
        app.clone(),
        "POST",
        "/oauth2/clients",
        &token,
        r#"{"client_name":"events","redirect_uris":[],"subscribed_events":[2.0,3e0,40e-1]}"#,
    )
    .await;
    assert_eq!(status, 201, "{registered}");
    let app_token = registered["registration_access_token"].as_str().unwrap();
    let (status, read) =
        request(app.clone(), "GET", "/oauth2/clients/@me", app_token, "null").await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["subscribed_events"], json!([2, 3, 4]));

    let (status, body) = request(
        app.clone(),
        "PATCH",
        "/oauth2/clients/@me",
        app_token,
        r#"{"subscribed_events":[4e0,2.0]}"#,
    )
    .await;
    assert_eq!(status, 204, "{body}");
    let (_, read) = request(app.clone(), "GET", "/oauth2/clients/@me", app_token, "null").await;
    assert_eq!(read["subscribed_events"], json!([4, 2]));

    for events in ["[2.5]", "[1e100]", "[5.0]"] {
        let (status, body) = request(
            app.clone(),
            "POST",
            "/oauth2/clients",
            &token,
            &format!(r#"{{"redirect_uris":[],"subscribed_events":{events}}}"#),
        )
        .await;
        assert!([400, 422].contains(&status), "{events}: {status} {body}");
        let (status, body) = request(
            app.clone(),
            "PATCH",
            "/oauth2/clients/@me",
            app_token,
            &format!(r#"{{"subscribed_events":{events}}}"#),
        )
        .await;
        assert_eq!(status, 400, "{events}: {body}");
        let (_, read) = request(app.clone(), "GET", "/oauth2/clients/@me", app_token, "null").await;
        assert_eq!(read["subscribed_events"], json!([4, 2]));
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn discord_commands_accept_integer_values_written_as_decimals(pool: PgPool) {
    support::setup_money(&pool).await;
    let discord = fake();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let mut payload = support::execute_from_guild(
        json!({"name":"pay","options":[
            {"name":"user","type":6.0,"value":"100000000000000002"},
            {"name":"amount","type":4.0,"value":20.0},
            {"name":"unit","type":3.0,"value":"n"}
        ]}),
        PARTY,
    );
    payload["type"] = json!(2.0);
    payload["application_id"] = json!("123");
    payload["token"] = json!("pay-token");
    let response = support::interaction(app, payload).await;
    // Pay returns its final result directly, including when Discord's integer
    // fields use decimal notation. The balances are committed before it replies.
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(response.body["type"], 4);
    assert!(discord.callbacks().is_empty());
    assert!(discord.webhooks().is_empty());
    assert!(discord.response_edits().is_empty());
    assert_eq!(support::get_amount(&pool, PARTY, 1).await, 199_480);
    assert_eq!(
        support::get_amount(&pool, 100_000_000_000_000_002, 1).await,
        1_020
    );
}
