use std::sync::Arc;

use sqlx::PgPool;
use vc_auth::AuthState;
use vc_core::notification::Notifier;

use crate::discord::DiscordApi;
use crate::notification::Proxy;
use crate::rate_limit::RateLimiter;
use crate::rate_limit::VerificationLimiter;

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

/// What the service signs with: the API's bearer tokens, and the browser's
/// session. Held together because they are the same kind of decision, and kept
/// as two *separate* secrets because both are JWTs — signing them with one
/// secret is what would let a session cookie be replayed as an API token.
#[derive(Clone)]
pub struct Signing {
    jwt: Arc<Vec<u8>>,
    session: Arc<Vec<u8>>,
    secure_session: bool,
}

impl Signing {
    pub fn new(jwt: impl Into<Vec<u8>>, session: impl Into<Vec<u8>>, secure_session: bool) -> Self {
        Self {
            jwt: Arc::new(jwt.into()),
            session: Arc::new(session.into()),
            secure_session,
        }
    }
}

/// What this service sends with: the proxy that reaches applications, and the
/// notifier built on it.
///
/// The two travel together because one is how the other works. `proxy` is `None`
/// when none is configured, which is development and any deployment that has not
/// been given the certificate — and the webhook handshake a registration performs
/// is the one thing that needs it rather than the notifier the deliveries use.
#[derive(Clone)]
pub struct Outbound {
    pub proxy: Option<Arc<Proxy>>,
    pub notifier: Arc<dyn Notifier>,
    /// The handshake's allowance. In the state rather than built where it is asked
    /// for, because a limiter per request is a limiter whose windows reset every
    /// time and therefore limit nothing.
    pub handshake: Arc<VerificationLimiter>,
}

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    signing: Signing,
    discord_public_key: Arc<[u8; 32]>,
    links: Arc<Links>,
    discord: Arc<dyn DiscordApi>,
    outbound: Outbound,
    limiter: Arc<RateLimiter>,
}

impl AppState {
    pub fn new(
        pool: PgPool,
        signing: Signing,
        discord_public_key: [u8; 32],
        links: Links,
        discord: Arc<dyn DiscordApi>,
        outbound: Outbound,
        limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            pool,
            signing,
            discord_public_key: Arc::new(discord_public_key),
            links: Arc::new(links),
            discord,
            outbound,
            limiter,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// What the API's own bearer tokens are signed with.
    pub fn jwt_secret(&self) -> &[u8] {
        &self.signing.jwt
    }

    /// What the browser's session cookie is signed with. Deliberately not the
    /// one above: a session cookie and an API token are both JWTs, and they must
    /// not be interchangeable.
    pub fn session_secret(&self) -> &[u8] {
        &self.signing.session
    }

    /// Whether the session cookie is marked `Secure`. Production wants it; a
    /// development server on plain HTTP can never be sent one.
    pub fn secure_cookies(&self) -> bool {
        self.signing.secure_session
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
        self.outbound.notifier.as_ref()
    }

    /// The proxy that reaches applications, when one is configured.
    ///
    /// `None` is a service that delivers nothing and verifies nothing: the claim
    /// paths still complete, and a registration that asks for a webhook has
    /// something this service cannot do.
    pub fn webhook_proxy(&self) -> Option<&Arc<Proxy>> {
        self.outbound.proxy.as_ref()
    }

    /// The allowance a webhook handshake is held to, which is per requester.
    pub fn handshake_limiter(&self) -> &VerificationLimiter {
        &self.outbound.handshake
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
        &self.signing.jwt
    }
}
