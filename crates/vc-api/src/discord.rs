use std::collections::BTreeMap;

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

    async fn refresh_token(&self, refresh_token: &str) -> Result<RefreshedToken, DiscordError>;
}

pub struct HttpDiscordApi {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    bot_token: String,
}

impl HttpDiscordApi {
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        bot_token: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            bot_token: bot_token.into(),
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
