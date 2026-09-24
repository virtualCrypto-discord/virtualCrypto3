use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sqlx::PgPool;
use vc_auth::AuthState;
use vc_core::notification::Notifier;

use crate::discord::DiscordApi;
use crate::notification::Transport;
use crate::rate_limit::RateLimiter;
use crate::rate_limit::VerificationLimiter;

/// The public URLs the command responses link to, plus the logo their screens show.
#[derive(Clone, Debug)]
pub struct Links {
    pub site_url: String,
    pub invite_url: String,
    pub support_guild_invite_url: String,
}

impl Links {
    /// `Command.logo_url/0`: where this deployment serves the project's logo.
    ///
    /// The path is the Elixir deployment's — Phoenix served `priv/static/images/logo.jpg` at
    /// `/static/images/logo.jpg` — and the same file is at `web/public/static/images/logo.jpg`
    /// here: Vite copies `public/` into the built site as it stands, and `vc-api` serves that
    /// directory, so a URL written for one deployment answers in the other.
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

/// The explicitly selected webhook transport and its notifier. The server
/// requires a proxy in production and permits Direct only in development;
/// request handlers never choose a fallback transport themselves.
#[derive(Clone)]
pub struct Outbound {
    pub transport: Arc<dyn Transport>,
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
    /// This application's commands and their ids, which a command mention is written with.
    /// Read from Discord once and remembered; see [`AppState::command_ids`].
    command_ids: Arc<CommandIds>,
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
            command_ids: Arc::new(CommandIds::default()),
        }
    }

    /// Command discovery gets at most 100ms of the response budget. A slow
    /// request continues in the background, and later responses use its result.
    /// Failed discovery is retried by a later call instead of caching an outage.
    pub async fn command_ids(&self) -> &BTreeMap<String, u64> {
        static EMPTY: BTreeMap<String, u64> = BTreeMap::new();
        let cache = &self.command_ids;
        if let Some(ids) = cache.ids.get() {
            return ids;
        }

        let ready = cache.ready.notified();
        tokio::pin!(ready);
        ready.as_mut().enable();
        if let Ok(guard) = cache.fetch.clone().try_lock_owned() {
            let cache = cache.clone();
            let discord = self.discord.clone();
            tokio::spawn(async move {
                let _guard = guard;
                if cache.ids.get().is_none()
                    && let Ok(Ok(commands)) = tokio::time::timeout(
                        Duration::from_secs(10),
                        discord.get_application_commands(),
                    )
                    .await
                {
                    let mut ids = BTreeMap::new();
                    for command in &commands {
                        command_ids(command, "", None, &mut ids);
                    }
                    let _ = cache.ids.set(ids);
                }
                cache.ready.notify_waiters();
            });
        }
        if cache.ids.get().is_none() {
            let _ = tokio::time::timeout(Duration::from_millis(100), ready).await;
        }
        cache.ids.get().unwrap_or(&EMPTY)
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

    /// The same transport that notifications use, selected at startup.
    pub fn webhook_transport(&self) -> &dyn Transport {
        self.outbound.transport.as_ref()
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

#[derive(Default)]
struct CommandIds {
    ids: OnceLock<BTreeMap<String, u64>>,
    fetch: Arc<tokio::sync::Mutex<()>>,
    ready: tokio::sync::Notify,
}

/// Subcommands and groups use their top-level command's id in every mention.
fn command_ids(
    command: &serde_json::Map<String, serde_json::Value>,
    prefix: &str,
    parent_id: Option<u64>,
    into: &mut std::collections::BTreeMap<String, u64>,
) {
    let Some(name) = command.get("name").and_then(serde_json::Value::as_str) else {
        return;
    };

    let path = if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix} {name}")
    };

    let id = parent_id.or_else(|| {
        command
            .get("id")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| id.parse().ok())
    });
    let Some(id) = id else { return };

    into.insert(path.clone(), id);

    // 1 is a subcommand and 2 a group of them; both take a place in the path, which is why
    // the walk does not care which it found.
    for option in command
        .get("options")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if matches!(
            option.get("type").and_then(serde_json::Value::as_i64),
            Some(1 | 2)
        ) && let Some(option) = option.as_object()
        {
            command_ids(option, &path, Some(id), into);
        }
    }
}
