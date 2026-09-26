//! Removed login entry points cannot mint a user credential.
mod support;
use axum::{body::Body, http::Request};
use sqlx::PgPool;
use support::{fake, links, state};
use tower::ServiceExt;

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn login_logout_and_token_are_retired(pool: PgPool) {
    let app = vc_api::router(state(pool.clone(), fake()));
    for (method, uri) in [
        ("GET", "/login?continue=https://untrusted.example/"),
        ("GET", "/logout"),
        ("POST", "/token"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("cookie", "_virtualcrypto_session=old-cookie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 410);
        assert!(!response.headers().contains_key("location"));
        assert!(!response.headers().contains_key("set-cookie"));
    }
    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM user_access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(tokens, 0);
    let sessions: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('browser_sessions')::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(sessions.is_none());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn public_redirects_are_unchanged(pool: PgPool) {
    let app = vc_api::router(state(pool, fake()));
    for (path, destination) in [
        ("/invite", links().invite_url),
        ("/support", links().support_guild_invite_url),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 307);
        assert_eq!(response.headers()["location"], destination);
    }
}

#[test]
fn real_discord_authorization_url_preserves_the_endpoint_and_parameters() {
    use vc_api::discord::{DiscordApi, HttpDiscordApi};
    let redirect = "https://vc.example/callback/discord?first=1&second=2";
    let state = "state+with&reserved=characters";
    let discord = HttpDiscordApi::new("123", "unused", "unused", redirect);
    let address = reqwest::Url::parse(&discord.authorize_url(state)).unwrap();
    assert_eq!(
        address.origin().ascii_serialization(),
        "https://discord.com"
    );
    assert_eq!(address.path(), "/api/oauth2/authorize");
    let parameters: std::collections::HashMap<_, _> = address.query_pairs().collect();
    assert_eq!(parameters.len(), 6);
    assert_eq!(parameters["client_id"], "123");
    assert_eq!(parameters["redirect_uri"], redirect);
    assert_eq!(parameters["state"], state);
    assert_eq!(parameters["response_type"], "code");
    assert_eq!(parameters["scope"], "identify");
    assert_eq!(parameters["prompt"], "none");
}
