//! Exercise the real HTTP adapter against local wire responses, including its
//! interaction with the cache. No Discord credentials or external service needed.
use super::*;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Mutex;

struct Wire {
    answers: Mutex<VecDeque<(u16, Value)>>,
    requests: Mutex<Vec<(String, String, axum::http::Method)>>,
}

struct Server {
    wire: Arc<Wire>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn answer(State(wire): State<Arc<Wire>>, request: Request) -> impl IntoResponse {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let body = axum::body::to_bytes(request.into_body(), 16_384)
        .await
        .unwrap();
    wire.requests
        .lock()
        .unwrap()
        .push((path, String::from_utf8(body.to_vec()).unwrap(), method));
    let (status, body) = wire
        .answers
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected HTTP request");
    (StatusCode::from_u16(status).unwrap(), axum::Json(body))
}

async fn server(answers: Vec<(u16, Value)>) -> (HttpDiscordApi, Server) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let wire = Arc::new(Wire {
        answers: Mutex::new(answers.into()),
        requests: Mutex::new(Vec::new()),
    });
    let app = axum::Router::new()
        .fallback(answer)
        .with_state(wire.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut client = HttpDiscordApi::new(
        "123",
        "dummy-secret",
        "dummy-bot",
        "https://vc.example/callback/discord",
    );
    client.api_base = format!("http://{address}/api");
    client.http = reqwest::Client::builder().no_proxy().build().unwrap();
    (client, Server { wire, task })
}

fn token() -> Value {
    json!({"access_token":"opaque-access-token", "refresh_token":"rotated-refresh-token", "expires_in":604800, "token_type":"Bearer", "scope":"identify"})
}

#[tokio::test]
async fn refresh_and_code_exchange_read_discords_wire_format() {
    let (client, server) = server(vec![(200, token()), (200, token())]).await;
    let refreshed = client.refresh_token("old-refresh").await.unwrap();
    let exchanged = client.exchange_code("the-code").await.unwrap();
    for result in [refreshed, exchanged] {
        assert_eq!(result.token, "opaque-access-token");
        assert_eq!(
            result.refresh_token.as_deref(),
            Some("rotated-refresh-token")
        );
        assert_eq!(result.expires_in, 604800);
    }
    let requests = server.wire.requests.lock().unwrap();
    assert_eq!(requests[0].0, "/api/oauth2/token");
    assert_eq!(requests[1].0, "/api/oauth2/token");
    assert!(requests[0].1.contains("grant_type=refresh_token"));
    assert!(requests[0].1.contains("refresh_token=old-refresh"));
    assert!(requests[1].1.contains("grant_type=authorization_code"));
    assert!(requests[1].1.contains("code=the-code"));
}

#[tokio::test]
async fn token_lifetimes_accept_integral_decimals_but_not_fractions() {
    let (client, _server) = server(vec![
        (200, json!({"access_token":"opaque", "expires_in":3600.0})),
        (200, json!({"access_token":"opaque", "expires_in":3600.5})),
    ])
    .await;
    assert_eq!(client.exchange_code("code").await.unwrap().expires_in, 3600);
    assert!(client.refresh_token("refresh").await.is_err());
}

#[tokio::test]
async fn token_responses_require_success_and_a_lifetime() {
    let (client, _server) = server(vec![
        (401, token()),
        (503, token()),
        (200, json!({"access_token":"opaque"})),
        (200, json!({"access_token":"opaque", "expires_in":3600})),
    ])
    .await;
    assert!(client.refresh_token("refresh").await.is_err());
    assert!(client.exchange_code("code").await.is_err());
    assert!(client.refresh_token("refresh").await.is_err());
    let refreshed = client.refresh_token("refresh").await.unwrap();
    assert_eq!(refreshed.token, "opaque");
    assert_eq!(refreshed.refresh_token, None);
}

#[tokio::test]
async fn user_and_guild_caches_retry_after_http_errors() {
    for status in [401, 403, 429, 500, 503] {
        for guild in [false, true] {
            let (client, server) = server(vec![
                (
                    status,
                    json!({"message":"temporary failure", "retry_after":0.01}),
                ),
                (
                    200,
                    json!({"id":"31414", "name":"recovered", "username":"recovered"}),
                ),
            ])
            .await;
            let cached = CachedDiscord::new(Arc::new(client));
            let first = if guild {
                cached.get_guild(31414).await
            } else {
                cached.get_user(31414).await
            };
            assert!(first.is_err(), "HTTP {status} was accepted as a resource");
            let second = if guild {
                cached.get_guild(31414).await
            } else {
                cached.get_user(31414).await
            };
            assert_eq!(second.unwrap().unwrap()["id"], "31414");
            let third = if guild {
                cached.get_guild(31414).await
            } else {
                cached.get_user(31414).await
            };
            assert_eq!(third.unwrap().unwrap()["id"], "31414");
            assert_eq!(server.wire.requests.lock().unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn not_found_is_still_cached_for_users_and_guilds() {
    for guild in [false, true] {
        let (client, server) = server(vec![(404, json!({"message":"Unknown resource"}))]).await;
        let cached = CachedDiscord::new(Arc::new(client));
        for _ in 0..2 {
            let result = if guild {
                cached.get_guild(123).await
            } else {
                cached.get_user(123).await
            };
            assert!(result.unwrap().is_none());
        }
        assert_eq!(server.wire.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn other_resource_readers_reject_http_errors() {
    let (client, _server) = server(vec![(429, json!({"message":"rate limited"})); 4]).await;
    assert!(client.get_user_info("token").await.is_err());
    assert!(client.get_guild_member(123, 456).await.is_err());
    assert!(client.get_roles(123).await.is_err());
    assert!(client.get_application_commands().await.is_err());
}

#[tokio::test]
async fn status_aware_readers_keep_the_status_for_their_callers() {
    let body = json!({"message":"Missing Permissions"});
    let (client, _server) = server(vec![(403, body.clone()); 3]).await;
    let (status, user) = client.get_user_with_status(123).await.unwrap();
    assert_eq!(status, 403);
    assert_eq!(user["message"], body["message"]);
    let (status, guild) = client.get_guild_with_status(123).await.unwrap();
    assert_eq!(status, 403);
    assert_eq!(guild["message"], body["message"]);
    assert_eq!(
        client
            .get_guild_integrations_with_status(123)
            .await
            .unwrap()
            .0,
        403
    );
}

#[tokio::test]
async fn interaction_callback_accepts_empty_204_and_rejects_errors() {
    let (client, server) = server(vec![(204, Value::Null), (404, json!({"code": 10062}))]).await;
    let cached = CachedDiscord::new(Arc::new(client));
    let body = json!({"type": 7, "data": {"content": "updated"}});
    cached
        .create_interaction_response("123", "test-token", &body)
        .await
        .unwrap();
    assert!(
        cached
            .create_interaction_response("123", "test-token", &body)
            .await
            .is_err()
    );
    let requests = server.wire.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].0, "/api/interactions/123/test-token/callback");
    assert_eq!(serde_json::from_str::<Value>(&requests[0].1).unwrap(), body);
}

#[tokio::test]
async fn original_interaction_response_is_patched_and_errors_are_sanitized() {
    let (client, server) = server(vec![
        (200, json!({"id":"789"})),
        (404, json!({"code":10015})),
    ])
    .await;
    let cached = CachedDiscord::new(Arc::new(client));
    let body = crate::components::ephemeral(vec![crate::components::text("done")]);
    cached
        .edit_original_interaction_response("123", "test-token", &body)
        .await
        .unwrap();
    let error = cached
        .edit_original_interaction_response("123", "test-token", &body)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("test-token"));
    let requests = server.wire.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].0,
        "/api/webhooks/123/test-token/messages/@original"
    );
    assert_eq!(requests[0].2, axum::http::Method::PATCH);
    assert_eq!(serde_json::from_str::<Value>(&requests[0].1).unwrap(), body);
}

#[tokio::test]
async fn original_response_deletion_uses_delete_and_sanitizes_errors() {
    let (client, server) = server(vec![(204, Value::Null), (403, json!({"code": 50013}))]).await;
    let cached = CachedDiscord::new(Arc::new(client));
    cached
        .delete_original_interaction_response("123", "test-token")
        .await
        .unwrap();
    let error = cached
        .delete_original_interaction_response("123", "test-token")
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("test-token"));
    let requests = server.wire.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].0,
        "/api/webhooks/123/test-token/messages/@original"
    );
    assert_eq!(requests[0].2, axum::http::Method::DELETE);
    assert!(requests[0].1.is_empty());
}

#[tokio::test]
async fn failed_lookups_release_flight_locks() {
    let entries = Entries::<i64>::new();
    for key in 0..100 {
        let result = entries
            .fetch(key, || async { Err::<Option<i64>, _>(()) })
            .await;
        assert!(result.is_err());
    }
    assert_eq!(
        entries.flights.lock().unwrap().len(),
        0,
        "finished failures must not retain per-key locks"
    );
}

#[tokio::test]
async fn waiting_cache_hits_release_flight_locks() {
    let entries = std::sync::Arc::new(Entries::<i64>::new());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let first_entries = entries.clone();
    let first = tokio::spawn(async move {
        first_entries
            .fetch(1, || async {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok::<_, ()>(Some(42))
            })
            .await
    });
    started_rx.await.unwrap();
    let waiting_entries = entries.clone();
    let waiting = tokio::spawn(async move {
        waiting_entries
            .fetch(1, || async { panic!("should read the cached value") })
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if std::sync::Arc::strong_count(entries.flights.lock().unwrap().get(&1).unwrap()) == 3 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(first.await.unwrap(), Ok(Some(42)));
    let answer: Result<Option<i64>, ()> = waiting.await.unwrap();
    assert_eq!(answer, Ok(Some(42)));
    assert_eq!(
        entries.flights.lock().unwrap().len(),
        0,
        "the final waiter must release the flight entry"
    );
}

#[tokio::test]
async fn cancelled_lookup_releases_flight_lock() {
    let entries = Arc::new(Entries::<i64>::new());
    let fetching = entries.clone();
    let (started, received) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        fetching
            .fetch(1, || async {
                started.send(()).unwrap();
                std::future::pending::<Result<Option<i64>, ()>>().await
            })
            .await
    });
    received.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(entries.flights.lock().unwrap().is_empty());
    assert_eq!(
        entries.fetch(1, || async { Ok::<_, ()>(Some(42)) }).await,
        Ok(Some(42))
    );
    assert!(entries.flights.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn guild_authority_uses_live_ownership_even_when_display_data_is_cached(pool: sqlx::PgPool) {
    use crate::permissions::{GuildAccess, guild_access};
    use crate::state::{AppState, Links, Outbound, Signing};

    let old_owner = vc_core::user::resolve_discord_id(&pool, 111).await.unwrap();
    let new_owner = vc_core::user::resolve_discord_id(&pool, 222).await.unwrap();
    let old_guild = json!({"id":"333", "owner_id":"111"});
    let new_guild = json!({"id":"333", "owner_id":"222"});
    let mut answers = vec![(200, old_guild.clone())];
    // Before transfer, after transfer for the old owner, then for the new owner.
    for guild in [old_guild.clone(), new_guild.clone(), new_guild] {
        answers.extend([
            (200, guild),
            (200, json!({"roles":[]})), // Bot is still a member.
            (200, json!({"roles":[]})), // Neither user has an administrator role.
            (200, json!([])),
        ]);
    }
    // A failed fresh lookup must not fall back to the cached owner.
    for status in [403, 404, 429, 500] {
        answers.push((status, old_guild.clone()));
    }
    let (client, server) = server(answers).await;
    let cached = Arc::new(CachedDiscord::new(Arc::new(client)));
    assert_eq!(
        cached.get_guild(333).await.unwrap().unwrap()["owner_id"],
        "111"
    );
    let state = AppState::new(
        pool,
        Signing::new(b"test-secret".to_vec(), false),
        [0; 32],
        Links {
            site_url: "https://vc.example".into(),
            invite_url: "https://vc.example/invite".into(),
            support_guild_invite_url: "https://vc.example/support".into(),
        },
        cached.clone(),
        Outbound {
            transport: Arc::new(crate::notification::Direct::default()),
            notifier: Arc::new(vc_core::notification::NoopNotifier),
            handshake: Arc::new(crate::rate_limit::VerificationLimiter::new()),
        },
        Arc::new(crate::rate_limit::RateLimiter::new(
            100,
            std::time::Duration::from_secs(60),
        )),
        Arc::new(crate::security::BehaviorMonitor::for_test()),
    );
    assert_eq!(
        guild_access(&state, 333, old_owner).await,
        GuildAccess::Permitted
    );
    assert_eq!(
        guild_access(&state, 333, old_owner).await,
        GuildAccess::Denied
    );
    assert_eq!(
        guild_access(&state, 333, new_owner).await,
        GuildAccess::Permitted
    );
    for _ in 0..4 {
        assert_eq!(
            guild_access(&state, 333, old_owner).await,
            GuildAccess::Unknown
        );
    }
    assert_eq!(
        cached.get_guild(333).await.unwrap().unwrap()["owner_id"],
        "111"
    );
    assert!(server.wire.answers.lock().unwrap().is_empty());
}
