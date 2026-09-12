use std::sync::Arc;

use sqlx::PgPool;
use vc_auth::AuthState;
use vc_core::notification::Notifier;

use crate::discord::DiscordApi;
use crate::rate_limit::RateLimiter;

/// The public URLs the command responses link to, plus the logo their embeds show.
#[derive(Clone, Debug)]
pub struct Links {
    pub site_url: String,
    pub invite_url: String,
    pub support_guild_invite_url: String,
}

impl Links {
    /// `Command.logo_url/0`: the site's logo, which Phoenix serves from `/static`.
    pub fn logo_url(&self) -> String {
        format!("{}/static/images/logo.jpg", self.site_url)
    }
}

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    jwt_secret: Arc<Vec<u8>>,
    discord_public_key: Arc<[u8; 32]>,
    links: Arc<Links>,
    discord: Arc<dyn DiscordApi>,
    notifier: Arc<dyn Notifier>,
    limiter: Arc<RateLimiter>,
}

impl AppState {
    pub fn new(
        pool: PgPool,
        jwt_secret: impl Into<Vec<u8>>,
        discord_public_key: [u8; 32],
        links: Links,
        discord: Arc<dyn DiscordApi>,
        notifier: Arc<dyn Notifier>,
        limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            pool,
            jwt_secret: Arc::new(jwt_secret.into()),
            discord_public_key: Arc::new(discord_public_key),
            links: Arc::new(links),
            discord,
            notifier,
            limiter,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn discord(&self) -> &Arc<dyn DiscordApi> {
        &self.discord
    }

    /// The per-user request allowance, held against whoever the request
    /// proved to be.
    pub fn limiter(&self) -> &RateLimiter {
        &self.limiter
    }

    /// The claim-update dispatcher, which a test replaces with its own sink.
    pub fn notifier(&self) -> &dyn Notifier {
        self.notifier.as_ref()
    }

    pub fn links(&self) -> &Links {
        &self.links
    }

    /// The Discord application's Ed25519 public key, used to verify that an
    /// interaction request really came from Discord.
    pub fn discord_public_key(&self) -> &[u8; 32] {
        &self.discord_public_key
    }
}

impl AuthState for AppState {
    fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn jwt_secret(&self) -> &[u8] {
        &self.jwt_secret
    }
}
