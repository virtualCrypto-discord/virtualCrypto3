//! Real browser regression: only the external Discord provider is faked.
mod support;

use axum::{http::HeaderMap, response::Html, routing::get};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use support::*;
use vc_api::{
    AppState,
    state::{Outbound, Signing},
};

#[sqlx::test(migrations = "../vc-core/migrations")]
#[ignore = "requires npm ci in tools/oauth-browser and Playwright Chromium; run in CI browser step"]
async fn consent_form_pkce_and_refresh_in_chromium(pool: PgPool) {
    setup_money(&pool).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    // A distinct client origin checks that callback query strings are not leaked.
    let redirect = format!("http://localhost:{port}/client/callback");
    let mut clients = vec![];
    for method in ["none", "client_secret_basic"] {
        let id = insert_application(&pool, MONEY_USER1, "Browser PKCE test").await;
        sqlx::query("UPDATE applications SET client_secret='browser-test-secret', token_endpoint_auth_method=$2::token_endpoint_auth_method, pkce_policy='required', grant_types=ARRAY['authorization_code','refresh_token']::openid_connect_grant_types[] WHERE id=$1")
            .bind(id).bind(method).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO redirect_uris(application_id,redirect_uri,inserted_at,updated_at) VALUES($1,$2,now(),now())")
            .bind(id).bind(&redirect).execute(&pool).await.unwrap();
        clients.push(json!({"id":client_id_of(&pool,id).await,"method":method}));
    }
    let mut discord = FakeDiscord::with_member(MONEY_USER1, &[], &[]);
    Arc::get_mut(&mut discord).unwrap().authorization_url =
        Some(format!("http://localhost:{port}/discord/authorize"));
    let callback_base = base.clone();
    let state = AppState::new(
        pool.clone(),
        Signing::new(JWT_SECRET, false),
        discord_public_key(),
        vc_api::state::Links {
            site_url: base.clone(),
            ..links()
        },
        discord,
        Outbound {
            transport: Arc::new(vc_api::notification::Direct::default()),
            notifier: Arc::new(vc_core::notification::NoopNotifier),
            handshake: Arc::new(vc_api::rate_limit::VerificationLimiter::new()),
        },
        Arc::new(vc_api::rate_limit::RateLimiter::new(
            0,
            vc_api::rate_limit::DEFAULT_WINDOW,
        )),
        Arc::new(vc_api::security::BehaviorMonitor::for_test()),
    );
    let app = vc_api::router(state)
    .route("/discord/authorize", get(move |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String,String>>| {
        let base = callback_base.clone();
        async move {
            let state = uuid::Uuid::parse_str(&query["state"]).unwrap();
            axum::response::Redirect::to(&format!("{base}/callback/discord?state={state}&code=test-discord-code&iss=https%3A%2F%2Fdiscord.com"))
        }
    }))
    .route("/test/null-origin", get(|axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String,String>>| async move {
        let flow = uuid::Uuid::parse_str(&query["flow_id"]).unwrap();
        ([("referrer-policy", "no-referrer")], Html(format!(r#"<form method="post" action="/oauth2/authorize"><input name="action" value="approve"><input name="flow_id" value="{flow}"><button type="submit">Approve</button></form>"#)))
    }))
    .route(
        "/client/callback",
        get(|headers: HeaderMap| async move {
            assert!(
                !headers.contains_key("referer"),
                "authorization redirect leaked its referrer"
            );
            Html("<!doctype html><title>Client callback</title>Returned to client")
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let fixture =
        json!({"base":base,"redirect":redirect,"guild":MONEY_GUILD.to_string(),"clients":clients});
    let result = tokio::task::spawn_blocking(move || {
        std::process::Command::new("node")
            .arg(format!(
                "{}/../../tools/oauth-browser/consent.mjs",
                env!("CARGO_MANIFEST_DIR")
            ))
            .env("OAUTH_BROWSER_FIXTURE", fixture.to_string())
            .output()
            .expect("Node.js must be installed")
    })
    .await
    .unwrap();
    server.abort();
    assert!(
        result.status.success(),
        "browser failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("{}", String::from_utf8_lossy(&result.stdout));
}
