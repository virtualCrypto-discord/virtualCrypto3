//! Tell Discord about the commands, which is what `mix run priv/register-commands.exs`
//! did for the site this replaces.
//!
//! Discord's `PUT` on this endpoint is a bulk overwrite, so this is both what puts the
//! commands there and what removes one that is no longer in
//! [`discord_commands::commands`]. It has to be run when the list changes; the service
//! answering an interaction is a separate thing and does not need it.
//!
//!     VCRYPTO_BOT_TOKEN=... VCRYPTO_CLIENT_ID=... cargo run -p vc-api --bin register-commands
//!     ... register-commands 123456789012345678    # one guild's commands instead
//!
//!     just register-commands

use vc_api::discord_commands;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = require_env("VCRYPTO_BOT_TOKEN")?;
    let client_id = require_env("VCRYPTO_CLIENT_ID")?;

    // An argument is a guild. The old script took one for the same reason: a guild's
    // commands appear in it at once, where an application's can take an hour, which is
    // worth having while the list is being written.
    let guild = match std::env::args().nth(1) {
        Some(guild) => Some(guild.parse::<i64>()?),
        None => None,
    };

    let status = discord_commands::register(
        &reqwest::Client::new(),
        discord_commands::API_BASE,
        &token,
        &client_id,
        guild,
    )
    .await?;

    // The status is the answer: 200 is every command as given, 401 is the token, 403 is
    // the application not being the bot's.
    println!("{status}");

    Ok(())
}

fn require_env(key: &str) -> Result<String, Box<dyn std::error::Error>> {
    std::env::var(key).map_err(|_| format!("environment variable {key} is required").into())
}
