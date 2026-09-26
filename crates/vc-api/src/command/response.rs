use std::{sync::Arc, time::Duration};

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use super::CommandError;
use crate::{discord::DiscordApi, state::AppState};

/// Run a private management operation only after Discord accepts its callback.
/// Commands and modal submissions create a private message; buttons update their
/// existing private screen. A type 4 refusal from a button remains a separate reply.
pub(crate) async fn run<F, Fut>(
    state: &AppState,
    payload: &Value,
    update: bool,
    operation: F,
) -> Result<Response, CommandError>
where
    F: FnOnce(AppState, Value) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Value, CommandError>> + Send,
{
    let reply = if update {
        Acknowledged::update(state, payload).await?
    } else {
        Acknowledged::private(state, payload).await?
    };
    let state = state.clone();
    let payload = payload.clone();
    tokio::spawn(async move {
        let response = operation(state, payload).await.unwrap_or_else(|error| {
            tracing::warn!(?error, "Discord management operation failed");
            json!({
                "type": super::CHANNEL_MESSAGE_WITH_SOURCE,
                "data": crate::components::ephemeral(vec![crate::components::container(
                    Some(super::COLOR_ERROR as u32),
                    vec![crate::components::text(
                        "処理結果を確認できませんでした。現在の状態を確認してください。"
                    )],
                )]),
            })
        });
        let body = response["data"].clone();
        if update
            && response["type"] == super::CHANNEL_MESSAGE_WITH_SOURCE
            && reply.followup(&body).await
        {
            return;
        }
        reply.edit(body).await;
    });
    Ok(StatusCode::ACCEPTED.into_response())
}

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
    /// The caller must use this only for an already private screen.
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
