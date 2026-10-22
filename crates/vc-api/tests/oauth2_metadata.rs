mod support;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use support::*;
use tower::ServiceExt;

#[tokio::test]
async fn discovery_is_public_json_and_uses_configured_urls_not_request_headers() {
    // No database connection is required to discover server capabilities.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://localhost/unused")
        .unwrap();
    let app = vc_api::router(state(pool, fake()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/oauth-authorization-server")
                .header("host", "evil.example")
                .header("forwarded", "host=evil.example;proto=http")
                .header("x-forwarded-host", "evil.example")
                .header("origin", "https://client.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert!(!response.headers().contains_key("set-cookie"));
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let issuer = links().site_url;
    assert_eq!(body["issuer"], issuer);
    for (key, path) in [
        ("authorization_endpoint", "/oauth2/authorize"),
        ("token_endpoint", "/oauth2/token"),
        ("revocation_endpoint", "/oauth2/token/revoke"),
        ("registration_endpoint", "/oauth2/clients"),
    ] {
        assert_eq!(body[key], format!("{issuer}{path}"));
    }
    assert_eq!(body["response_types_supported"], json!(["code"]));
    assert_eq!(body["response_modes_supported"], json!(["query"]));
    assert_eq!(
        body["grant_types_supported"],
        json!([
            "authorization_code",
            "refresh_token",
            "client_credentials",
            "urn:ietf:params:oauth:grant-type:device_code"
        ])
    );
    assert_eq!(
        body["token_endpoint_auth_methods_supported"],
        json!(["none", "client_secret_basic"])
    );
    assert_eq!(
        body["revocation_endpoint_auth_methods_supported"],
        json!(["none"])
    );
    assert_eq!(body["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(body["authorization_response_iss_parameter_supported"], true);
    assert!(body.get("jwks_uri").is_none());
}
