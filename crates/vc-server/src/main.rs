use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use vc_api::AppState;
use vc_api::discord::HttpDiscordApi;
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

    let pool = vc_core::db::connect(&database_url, 10).await?;
    let state = AppState::new(
        pool,
        jwt_secret,
        discord_public_key,
        links,
        Arc::new(HttpDiscordApi::new(
            discord_client_id,
            discord_client_secret,
            discord_bot_token,
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
