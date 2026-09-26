use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};

use super::CommandError;
use crate::{discord::DiscordApi, state::AppState};

/// A successful callback must precede any mutation. Delivery after that point
/// may fail, but must never cause the mutation to run again.
pub(super) struct Acknowledged {
    discord: Arc<dyn DiscordApi>,
    application_id: String,
    token: String,
}

impl Acknowledged {
    pub async fn private(state: &AppState, payload: &Value) -> Result<Self, CommandError> {
        Self::send(
            state,
            payload,
            json!({
                "type": super::CHANNEL_MESSAGE_WITH_SOURCE,
                "data": {
                    "flags": crate::components::EPHEMERAL,
                    "content": "処理中…",
                    "allowed_mentions": {"parse": []},
                },
            }),
        )
        .await
    }

    /// Type 6 acknowledges a component without replacing its source message.
    /// The claim and contract screens are already ephemeral.
    pub async fn update(state: &AppState, payload: &Value) -> Result<Self, CommandError> {
        Self::send(state, payload, json!({"type": 6})).await
    }

    async fn send(state: &AppState, payload: &Value, body: Value) -> Result<Self, CommandError> {
        let field = |name| {
            payload
                .get(name)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| CommandError::missing(&format!("interaction has no {name}")))
        };
        let id = field("id")?;
        let response = Self {
            discord: state.discord().clone(),
            application_id: field("application_id")?.to_owned(),
            token: field("token")?.to_owned(),
        };
        tokio::time::timeout(
            Duration::from_secs(2),
            response
                .discord
                .create_interaction_response(id, &response.token, &body),
        )
        .await
        .map_err(|_| CommandError::missing("interaction acknowledgement timed out"))??;
        Ok(response)
    }

    pub async fn edit(&self, mut body: Value) {
        // Privacy belongs to the existing message; this flag only enables V2.
        body["flags"] = json!(crate::components::IS_COMPONENTS_V2);
        body["content"] = Value::Null;
        body["embeds"] = json!([]);
        match tokio::time::timeout(
            Duration::from_secs(10),
            self.discord.edit_original_interaction_response(
                &self.application_id,
                &self.token,
                &body,
            ),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "interaction response edit failed"),
            Err(_) => tracing::warn!("interaction response edit timed out"),
        }
    }

    pub async fn followup(&self, body: &Value) -> bool {
        match tokio::time::timeout(
            Duration::from_secs(10),
            self.discord
                .post_webhook_message(&self.application_id, &self.token, body),
        )
        .await
        {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                tracing::warn!(%error, "interaction follow-up failed");
                false
            }
            Err(_) => {
                tracing::warn!("interaction follow-up timed out");
                false
            }
        }
    }
}
