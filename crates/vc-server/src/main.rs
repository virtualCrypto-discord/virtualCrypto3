use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use vc_api::AppState;
use vc_api::discord::{CachedDiscord, HttpDiscordApi};
use vc_api::state::Links;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let database_url = require_env("DATABASE_URL")?;
    let jwt_secret = require_env("GUARDIAN_SECRET_KEY")?;
    let discord_client_id = require_env("DISCORD_CLIENT_ID")?;
    let discord_client_secret = require_env("DISCORD_CLIENT_SECRET")?;
    let discord_bot_token = require_env("DISCORD_BOT_TOKEN")?;
    let discord_public_key = vc_api::discord::parse_public_key(&require_env("DISCORD_PUBLIC_KEY")?)
        .ok_or_else(|| "DISCORD_PUBLIC_KEY must be 32 hex-encoded bytes".to_string())?;
    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8080);

    // Elixir keeps these in `config/*.exs`; they are not secrets, so the defaults
    // match the ones the checked-in examples use.
    let links = Links {
        site_url: optional_env("SITE_URL", "https://vcrypto.sumidora.com"),
        invite_url: std::env::var("INVITE_URL").unwrap_or_else(|_| {
            format!(
                "https://discord.com/api/oauth2/authorize?client_id={discord_client_id}\
                 &permissions=0&scope=applications.commands%20bot"
            )
        }),
        support_guild_invite_url: optional_env(
            "SUPPORT_GUILD_INVITE_URL",
            "https://discord.com/invite/Hgp5DpG",
        ),
    };

    // A session that cannot be signed is not a session, so this secret is
    // required rather than defaulted to something convenient.
    let session_secret = require_env("SESSION_SECRET")?;
    // Secure unless switched off, because the alternative fails quietly: a
    // missing flag would send session cookies in the clear.
    let secure_cookies = optional_env("SECURE_COOKIES", "true") != "false";

    let pool = vc_core::db::connect(&database_url, 10).await?;

    // The webhook transport, when there is a proxy to send through. With none —
    // in development, or before the certificates exist — nothing is delivered and
    // claims still complete, which is what the no-op is for.
    let notifier: Arc<dyn vc_core::notification::Notifier> = match webhook_proxy() {
        Some(proxy) => Arc::new(vc_api::notification::WebhookNotifier::new(
            pool.clone(),
            proxy,
        )),
        None => Arc::new(vc_core::notification::NoopNotifier),
    };

    let state = AppState::new(
        pool,
        vc_api::state::Signing::new(jwt_secret, session_secret, secure_cookies),
        discord_public_key,
        links,
        // `Discord.Api.Cached`: the same lookups, remembered for fifteen
        // minutes, a 404 included.
        Arc::new(CachedDiscord::new(Arc::new(HttpDiscordApi::new(
            discord_client_id,
            discord_client_secret,
            discord_bot_token,
            // Where Discord sends the browser back to. It has to match the URL
            // the login redirect used, so a deployment sets its own origin.
            std::env::var("DISCORD_OAUTH2_REDIRECT_URI")
                .unwrap_or_else(|_| "http://localhost:8080/callback/discord".to_string()),
        )))),
        notifier.clone(),
        // A loose per-user allowance; `RATE_LIMIT_PER_MINUTE=0` turns it off.
        Arc::new(vc_api::rate_limit::RateLimiter::new(
            std::env::var("RATE_LIMIT_PER_MINUTE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(vc_api::rate_limit::DEFAULT_LIMIT),
            vc_api::rate_limit::DEFAULT_WINDOW,
        )),
    );

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;

    tracing::info!(%addr, "virtualCrypto server listening");

    axum::serve(listener, vc_api::router(state)).await?;

    Ok(())
}

fn require_env(key: &str) -> Result<String, Box<dyn std::error::Error>> {
    std::env::var(key).map_err(|_| format!("environment variable {key} is required").into())
}

fn optional_env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// The proxy to deliver webhooks through, if one is configured.
///
/// The certificate and its key arrive with `#` standing in for a newline, which
/// is how they are set: a PEM in an environment variable is otherwise a single
/// line of nothing. That trick is the Elixir's, and the deployment sets them the
/// same way.
///
/// No proxy at all is development, and is answered with `None`. A proxy whose
/// certificate will not parse is a configuration mistake and panics, because a
/// server that boots without ever delivering is worse than one that says so.
fn webhook_proxy() -> Option<vc_api::notification::Proxy> {
    let url = std::env::var("WEBHOOK_PROXY_URL").ok()?;
    let certificate = std::env::var("VCRYPTO_WEBHOOK_PROXY_CERT")
        .ok()?
        .replace('#', "\n");
    let key = std::env::var("VCRYPTO_WEBHOOK_PROXY_KEY")
        .ok()?
        .replace('#', "\n");

    Some(
        vc_api::notification::Proxy::new(url, certificate.as_bytes(), key.as_bytes())
            .expect("the webhook proxy certificate could not be used"),
    )
}
