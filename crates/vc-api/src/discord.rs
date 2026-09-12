use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The fields `VirtualCryptoWeb.Filtering.Discord.user/1` keeps. Order is the
/// Elixir list; serialization sorts them so the output matches the goldens.
pub const FILTERED_FIELDS: [&str; 9] = [
    "id",
    "username",
    "discriminator",
    "avatar",
    "bot",
    "system",
    "mfa_enabled",
    "premium_type",
    "public_flags",
];

#[derive(Debug, thiserror::Error)]
pub enum DiscordError {
    #[error("discord request failed: {0}")]
    Request(String),
}

/// The result of redeeming a Discord refresh token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshedToken {
    pub token: String,
    pub expires_in: i64,
    pub refresh_token: Option<String>,
}

/// Discord API access, injectable so tests do not call the network. This mirrors
/// the Elixir `DiscordApiService` seam, but `get_user_info/1` had no such seam
/// there, which is why the goldens needed a patched clone.
#[async_trait]
pub trait DiscordApi: Send + Sync {
    async fn get_user_info(&self, token: &str) -> Result<Map<String, Value>, DiscordError>;

    /// `Discord.Api.Cached.get_user/2`, used to decorate claims with the claimant
    /// and payer. `None` stands for the `:not_found` the Elixir cache stores,
    /// which the serializer then fails on.
    async fn get_user(
        &self,
        discord_user_id: i64,
    ) -> Result<Option<Map<String, Value>>, DiscordError>;

    /// `Discord.Api.Cached.get_guild/2`, which decorates the `info` embed with
    /// the currency's guild. `None` stands for the `:not_found` the Elixir cache
    /// stores after a 404.
    async fn get_guild(&self, guild_id: i64) -> Result<Option<Map<String, Value>>, DiscordError>;

    /// `Discord.Api.Raw.get_guild_member_with_status_code/2`, which the consent
    /// screen uses to ask whether the logged-in user may act for a guild. A 404
    /// is `None`, since "not a member" is an answer rather than a failure.
    ///
    /// Deliberately not cached, like the rest of `Raw`: a permission question
    /// answered from fifteen minutes ago is a permission question answered wrongly.
    async fn get_guild_member(
        &self,
        guild_id: i64,
        user_id: i64,
    ) -> Result<Option<Map<String, Value>>, DiscordError>;

    /// `Discord.Api.Raw.get_roles/1`: the guild's roles, whose permissions are
    /// what a member's own permissions are ORed together from.
    async fn get_roles(&self, guild_id: i64) -> Result<Vec<Map<String, Value>>, DiscordError>;

    /// `post_webhook_message/3`: the follow-up a component answers with, which
    /// is where a button's result is shown. The Elixir tests swap the service
    /// for one that records the body instead of sending it.
    async fn post_webhook_message(
        &self,
        application_id: &str,
        token: &str,
        body: &Value,
    ) -> Result<(), DiscordError>;

    async fn refresh_token(&self, refresh_token: &str) -> Result<RefreshedToken, DiscordError>;

    /// Redeeming an authorization code, the other half of the flow the web UI
    /// drives. Discord answers both grants with the same document, which is why
    /// this returns what a refresh does.
    async fn exchange_code(&self, code: &str) -> Result<RefreshedToken, DiscordError>;

    /// Where to send a browser so it can authorize. It belongs here rather than
    /// in configuration because the client id and the redirect URI it has to
    /// agree with are already this client's business, and because a test can
    /// then recognize the URL without building one.
    fn authorize_url(&self, state: &str) -> String;
}

/// How long a Discord lookup is remembered, matching the Elixir cache's TTL.
pub const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// A ceiling on the cache. Elixir's `Cachex` is the same shape — a table in the
/// process — but this one is bounded, so a long-lived server cannot grow
/// without limit on a busy guild.
pub const CACHE_LIMIT: usize = 10_000;

/// A TTL map whose value is itself an `Option`: `Some(None)` is a remembered
/// "not found", which `Cachex` stores as `:not_found` and answers from.
struct Entries<T> {
    entries: std::sync::Mutex<std::collections::HashMap<i64, (std::time::Instant, Option<T>)>>,
    /// One lock per id currently being fetched. Two callers who miss the same id
    /// therefore make one call between them, which is what the Elixir cache's
    /// per-key `Cachex.transaction!` does.
    flights:
        std::sync::Mutex<std::collections::HashMap<i64, std::sync::Arc<tokio::sync::Mutex<()>>>>,
}

impl<T: Clone> Entries<T> {
    fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(std::collections::HashMap::new()),
            flights: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The call for `key`, made once even when several callers want it at once.
    async fn fetch<F, Fut, E>(&self, key: i64, fetch: F) -> Result<Option<T>, E>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Option<T>, E>>,
    {
        if let Some(cached) = self.get(key) {
            return Ok(cached);
        }

        let flight = {
            let mut flights = self.flights.lock().expect("the cache is not poisoned");
            std::sync::Arc::clone(flights.entry(key).or_default())
        };

        // Held for the whole call, so a second caller waits here and then finds
        // what the first one stored.
        let held = flight.lock().await;

        if let Some(cached) = self.get(key) {
            return Ok(cached);
        }

        let value = fetch().await?;
        self.put(key, value.clone());

        drop(held);

        // Forget the lock once nobody is waiting on it, so the table is only as
        // large as the calls in flight.
        let mut flights = self.flights.lock().expect("the cache is not poisoned");
        let finished = flights
            .get(&key)
            .is_some_and(|flight| std::sync::Arc::strong_count(flight) == 2);

        if finished {
            flights.remove(&key);
        }

        Ok(value)
    }

    /// `Some(value)` is a hit, including a remembered miss; `None` means the
    /// lookup has to be made.
    fn get(&self, key: i64) -> Option<Option<T>> {
        let entries = self.entries.lock().expect("the cache is not poisoned");

        match entries.get(&key) {
            Some((at, value)) if at.elapsed() < CACHE_TTL => Some(value.clone()),
            _ => None,
        }
    }

    fn put(&self, key: i64, value: Option<T>) {
        let mut entries = self.entries.lock().expect("the cache is not poisoned");

        if entries.len() >= CACHE_LIMIT && !entries.contains_key(&key) {
            // Drop what has expired first: on a cache that has been running a
            // while that is most of it, and it costs one pass.
            entries.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);

            if entries.len() >= CACHE_LIMIT {
                // Still full, so give up the oldest tenth in one go. Evicting a
                // single entry would make every insert scan, and clearing the
                // table outright would send every caller back to Discord at
                // once.
                let mut stored: Vec<(std::time::Instant, i64)> =
                    entries.iter().map(|(key, (at, _))| (*at, *key)).collect();
                stored.sort_unstable();

                for (_, key) in stored.into_iter().take(CACHE_LIMIT / 10) {
                    entries.remove(&key);
                }
            }
        }

        entries.insert(key, (std::time::Instant::now(), value));
    }
}

/// `Discord.Api.Cached`: the same lookups as the wrapped API, remembered.
///
/// A 404 is remembered too, which is what keeps a page of claims from asking
/// Discord about the same missing user once per row.
pub struct CachedDiscord {
    inner: Arc<dyn DiscordApi>,
    users: Entries<Map<String, Value>>,
    guilds: Entries<Map<String, Value>>,
}

impl CachedDiscord {
    pub fn new(inner: Arc<dyn DiscordApi>) -> Self {
        Self {
            inner,
            users: Entries::new(),
            guilds: Entries::new(),
        }
    }
}

#[async_trait]
impl DiscordApi for CachedDiscord {
    /// Not cached: the Elixir cache wraps `get_user` and `get_guild` only, and a
    /// token is looked up once per request.
    async fn get_user_info(&self, token: &str) -> Result<Map<String, Value>, DiscordError> {
        self.inner.get_user_info(token).await
    }

    async fn get_user(
        &self,
        discord_user_id: i64,
    ) -> Result<Option<Map<String, Value>>, DiscordError> {
        self.users
            .fetch(discord_user_id, || self.inner.get_user(discord_user_id))
            .await
    }

    async fn get_guild(&self, guild_id: i64) -> Result<Option<Map<String, Value>>, DiscordError> {
        self.guilds
            .fetch(guild_id, || self.inner.get_guild(guild_id))
            .await
    }

    async fn get_guild_member(
        &self,
        guild_id: i64,
        user_id: i64,
    ) -> Result<Option<Map<String, Value>>, DiscordError> {
        self.inner.get_guild_member(guild_id, user_id).await
    }

    async fn get_roles(&self, guild_id: i64) -> Result<Vec<Map<String, Value>>, DiscordError> {
        self.inner.get_roles(guild_id).await
    }

    async fn post_webhook_message(
        &self,
        application_id: &str,
        token: &str,
        body: &Value,
    ) -> Result<(), DiscordError> {
        self.inner
            .post_webhook_message(application_id, token, body)
            .await
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<RefreshedToken, DiscordError> {
        self.inner.refresh_token(refresh_token).await
    }

    async fn exchange_code(&self, code: &str) -> Result<RefreshedToken, DiscordError> {
        self.inner.exchange_code(code).await
    }

    fn authorize_url(&self, state: &str) -> String {
        self.inner.authorize_url(state)
    }
}

pub struct HttpDiscordApi {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    bot_token: String,
    /// Discord requires this to match the URL it redirected the browser to, so
    /// it is configuration rather than something the exchange can infer.
    redirect_uri: String,
}

impl HttpDiscordApi {
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        bot_token: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            bot_token: bot_token.into(),
            redirect_uri: redirect_uri.into(),
        }
    }
}

#[async_trait]
impl DiscordApi for HttpDiscordApi {
    async fn get_user_info(&self, token: &str) -> Result<Map<String, Value>, DiscordError> {
        let response = self
            .http
            .get("https://discord.com/api/users/@me")
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        let body: Value = response
            .json()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        match body {
            Value::Object(map) => Ok(map),
            other => Err(DiscordError::Request(format!(
                "expected a JSON object, got {other}"
            ))),
        }
    }

    async fn get_user(
        &self,
        discord_user_id: i64,
    ) -> Result<Option<Map<String, Value>>, DiscordError> {
        let response = self
            .http
            .get(format!("https://discord.com/api/users/{discord_user_id}"))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bot {}", self.bot_token),
            )
            .send()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        // `Discord.Api.Raw` reports 404s as `:not_found`, which the user cache
        // stores and the claim serializer then fails on.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let body: Value = response
            .json()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        match body {
            Value::Object(map) => Ok(Some(map)),
            other => Err(DiscordError::Request(format!(
                "expected a JSON object, got {other}"
            ))),
        }
    }

    async fn get_guild_member(
        &self,
        guild_id: i64,
        user_id: i64,
    ) -> Result<Option<Map<String, Value>>, DiscordError> {
        let response = self
            .http
            .get(format!(
                "https://discord.com/api/guilds/{guild_id}/members/{user_id}"
            ))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bot {}", self.bot_token),
            )
            .send()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let body: Value = response
            .json()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        match body {
            Value::Object(member) => Ok(Some(member)),
            _ => Err(DiscordError::Request("member is not an object".into())),
        }
    }

    async fn get_roles(&self, guild_id: i64) -> Result<Vec<Map<String, Value>>, DiscordError> {
        let response = self
            .http
            .get(format!("https://discord.com/api/guilds/{guild_id}/roles"))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bot {}", self.bot_token),
            )
            .send()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        let body: Value = response
            .json()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        Ok(body
            .as_array()
            .map(|roles| {
                roles
                    .iter()
                    .filter_map(|role| role.as_object().cloned())
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn get_guild(&self, guild_id: i64) -> Result<Option<Map<String, Value>>, DiscordError> {
        let response = self
            .http
            .get(format!(
                "https://discord.com/api/guilds/{guild_id}?with_counts=false"
            ))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bot {}", self.bot_token),
            )
            .send()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        // `Discord.Api.GuildCache` records a 404 as `:not_found`.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let body: Value = response
            .json()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        match body {
            Value::Object(map) => Ok(Some(map)),
            other => Err(DiscordError::Request(format!(
                "expected a JSON object, got {other}"
            ))),
        }
    }

    async fn post_webhook_message(
        &self,
        application_id: &str,
        token: &str,
        body: &Value,
    ) -> Result<(), DiscordError> {
        let response = self
            .http
            .post(format!(
                "https://discord.com/api/webhooks/{application_id}/{token}"
            ))
            .json(body)
            .send()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        if !response.status().is_success() {
            return Err(DiscordError::Request(format!(
                "the webhook answered {}",
                response.status()
            )));
        }

        Ok(())
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<RefreshedToken, DiscordError> {
        let response = self
            .http
            .post("https://discord.com/api/oauth2/token")
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
            ])
            .send()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        let body: Value = response
            .json()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        // The Elixir code does `Jason.decode!(client.token.access_token)` and reads
        // the token out of that document, so the access_token is itself JSON here.
        // Reproduce that shape exactly rather than treating it as an opaque token.
        let access_token = body["access_token"]
            .as_str()
            .ok_or_else(|| DiscordError::Request("response has no access_token".into()))?;

        serde_json::from_str(access_token)
            .map_err(|error| DiscordError::Request(format!("access_token is not JSON: {error}")))
    }

    async fn exchange_code(&self, code: &str) -> Result<RefreshedToken, DiscordError> {
        let response = self
            .http
            .post("https://discord.com/api/oauth2/token")
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.redirect_uri),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
            ])
            .send()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        let body: Value = response
            .json()
            .await
            .map_err(|error| DiscordError::Request(error.to_string()))?;

        // Unlike a refresh, whose access_token arrives as a document the Elixir
        // code then decodes, this one is an opaque token and is read directly.
        Ok(RefreshedToken {
            token: body["access_token"]
                .as_str()
                .ok_or_else(|| DiscordError::Request("response has no access_token".into()))?
                .to_owned(),
            expires_in: body["expires_in"].as_i64().unwrap_or_default(),
            refresh_token: body["refresh_token"].as_str().map(str::to_owned),
        })
    }

    /// The parameters are the old app's, spelled out: `scope=identify`, because
    /// the bot only ever needs to know who someone is, and `prompt=none`,
    /// because Discord should not ask again someone who already agreed.
    fn authorize_url(&self, state: &str) -> String {
        format!(
            "https://discord.com/api/oauth2/authorize             ?client_id={}&redirect_uri={}&response_type=code&scope=identify&prompt=none&state={}",
            self.client_id, self.redirect_uri, state
        )
    }
}

/// `Map.take/2` semantics: keep a key when it is present, even if its value is
/// null; drop unknown keys entirely.
pub fn filter_profile(payload: Map<String, Value>) -> BTreeMap<String, Value> {
    FILTERED_FIELDS
        .iter()
        .filter_map(|field| {
            payload
                .get(*field)
                .map(|value| ((*field).to_string(), value.clone()))
        })
        .collect()
}

/// The Discord interaction handshake, as `InteractionsController.verify/1`
/// implements it: the signature is lowercase hex over `timestamp <> body`, and
/// the application's Ed25519 public key is configured in hex.
pub fn verify_signature(
    public_key: &[u8; 32],
    signature_hex: &str,
    timestamp: &str,
    body: &[u8],
) -> bool {
    let Ok(signature) = hex::decode(signature_hex) else {
        return false;
    };
    let Ok(signature) = ed25519_dalek::Signature::from_slice(&signature) else {
        return false;
    };
    let Ok(public_key) = ed25519_dalek::VerifyingKey::from_bytes(public_key) else {
        return false;
    };

    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);

    public_key.verify_strict(&message, &signature).is_ok()
}

/// Parse the configured hex-encoded Ed25519 public key.
pub fn parse_public_key(value: &str) -> Option<[u8; 32]> {
    hex::decode(value).ok()?.try_into().ok()
}
