use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use vc_api::AppState;
use vc_api::discord::{CachedDiscord, HttpDiscordApi};
use vc_api::state::Links;

mod outbound;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // The names are the deployment's own: the `VCRYPTO_*` secrets the app already
    // carries, which is what the Elixir reads — plus `DATABASE_URL` and
    // `SECRET_KEY_BASE`, which are Phoenix's and sqlx's. They are read as they are,
    // each on its own: a value the environment does not have is refused by name.
    let database_url = require_env("DATABASE_URL")?;
    let jwt_secret = require_env("VCRYPTO_API_JWT_SECRET_KEY")?;
    let discord_client_id = require_env("VCRYPTO_CLIENT_ID")?;
    let discord_client_secret = require_env("VCRYPTO_CLIENT_SECRET")?;
    let discord_bot_token = require_env("VCRYPTO_BOT_TOKEN")?;
    let discord_public_key = vc_api::discord::parse_public_key(&require_env("VCRYPTO_PUBLIC_KEY")?)
        .ok_or_else(|| "VCRYPTO_PUBLIC_KEY must be 32 hex-encoded bytes".to_string())?;
    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8080);

    // Elixir keeps these in `config/*.exs`; they are not secrets, so the defaults
    // match the ones the checked-in examples use.
    let links = Links {
        site_url: optional_env("VCRYPTO_SITE_URL", "https://vcrypto.sumidora.com"),
        invite_url: std::env::var("VCRYPTO_INVITE_URL").unwrap_or_else(|_| {
            format!(
                "https://discord.com/api/oauth2/authorize?client_id={discord_client_id}\
                 &permissions=0&scope=applications.commands%20bot"
            )
        }),
        support_guild_invite_url: optional_env(
            "VCRYPTO_SUPPORT_GUILD_INVITE_URL",
            "https://discord.com/invite/Hgp5DpG",
        ),
    };

    // A session that cannot be signed is not a session, so this secret is
    // required rather than defaulted to something convenient.
    let session_secret = require_env("SECRET_KEY_BASE")?;
    // Secure unless switched off, because the alternative fails quietly: a
    // missing flag would send session cookies in the clear.
    let secure_cookies = optional_env("SECURE_COOKIES", "true") != "false";

    // Validate outbound policy before connecting to the database or starting
    // jobs. Production is the default and cannot fall back to direct webhooks.
    let environment = outbound::Environment::parse(std::env::var("VCRYPTO_ENV").ok().as_deref())?;
    let transport = outbound::transport(
        environment,
        &optional_env(
            "WEBHOOK_PROXY_URL",
            "https://vcrypto-webhook-emitter.sumidora.com",
        ),
        std::env::var("VCRYPTO_WEBHOOK_PROXY_CERT").ok().as_deref(),
        std::env::var("VCRYPTO_WEBHOOK_PROXY_KEY").ok().as_deref(),
    )?;

    let pool = vc_core::db::connect(&database_url, 10).await?;

    let notifier: Arc<dyn vc_core::notification::Notifier> = Arc::new(
        vc_api::notification::WebhookNotifier::new(pool.clone(), Arc::clone(&transport)),
    );

    // Watching for behaviour worth a warning: how long a subject's counts are
    // held, the tighter observation-only window that fires before a 429, and
    // where the warnings go besides the log. Nothing here refuses a request.
    let monitor = Arc::new(
        vc_api::security::BehaviorMonitor::new(
            Duration::from_secs(
                std::env::var("VCRYPTO_SECURITY_WINDOW_SECS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(300),
            ),
            std::env::var("VCRYPTO_WATCH_LIMIT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(30),
            Duration::from_secs(
                std::env::var("VCRYPTO_WATCH_WINDOW_SECS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(10),
            ),
            std::env::var("VCRYPTO_REQUEST_SURGE_PER_MIN")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(3000),
        )
        .with_webhook(
            pool.clone(),
            std::env::var("VCRYPTO_SECURITY_WEBHOOK_URL").ok(),
        ),
    );

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
            std::env::var("VCRYPTO_DISCORD_CALLBACK_URI")
                .unwrap_or_else(|_| "http://localhost:8080/callback/discord".to_string()),
        )))),
        vc_api::state::Outbound {
            transport,
            notifier,
            handshake: Arc::new(vc_api::rate_limit::VerificationLimiter::new()),
        },
        // A loose per-user allowance; `RATE_LIMIT_PER_MINUTE=0` turns it off.
        Arc::new(vc_api::rate_limit::RateLimiter::new(
            std::env::var("RATE_LIMIT_PER_MINUTE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(vc_api::rate_limit::DEFAULT_LIMIT),
            vc_api::rate_limit::DEFAULT_WINDOW,
        )),
        Arc::clone(&monitor),
    );

    // The sweep that ends each observation window and sends whatever it owes.
    // Its own task rather than the scheduler's, so `VCRYPTO_SETTLE_INTERVAL_SECS=0`
    // — which silences contract settlement — does not silence these warnings.
    tokio::spawn(monitor.run());

    // The clock, which the state does not carry because it is not asked anything:
    // a contract whose deadline has passed is settled here, so the parties' money
    // comes home without any of them having to come back for it, and the
    // application that wrote it is told. `VCRYPTO_SETTLE_INTERVAL_SECS=0` turns it
    // off.
    tokio::spawn(vc_api::scheduler::run(
        state.clone(),
        vc_api::scheduler::interval(),
    ));

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
