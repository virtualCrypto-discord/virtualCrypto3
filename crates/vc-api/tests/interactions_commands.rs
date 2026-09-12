//! The `help` and `invite` commands, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/help_test.exs` and
//! `invite_test.exs`.

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{Response, fake, sign_interaction, state};
use tower::ServiceExt;

const URI: &str = "/api/integrations/discord/interactions";
const SITE_URL: &str = "https://vcrypto.sumidora.com";
const BOT_INVITE_URL: &str = "https://discord.com/api/oauth2/authorize?client_id=791984306632654869&permissions=0&scope=applications.commands%20bot";
const SUPPORT_GUILD_INVITE_URL: &str = "https://discord.com/invite/Hgp5DpG";

/// `InteractionsControllerTest.Helper.Common.execute_from_guild/2` with its
/// default guild and permissions.
fn execute_from_guild(data: Value, user: i64) -> Value {
    json!({
        "type": 2,
        "data": data,
        "member": {
            "user": { "id": user.to_string() },
            "permissions": "18446744073709551615",
        },
        "guild_id": "494780225280802817",
    })
}

async fn interaction(app: Router, payload: Value) -> Response {
    let body = serde_json::to_vec(&payload).expect("encode body");
    let (timestamp, signature) = sign_interaction(&body);

    let request = Request::builder()
        .method("POST")
        .uri(URI)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .header("x-signature-timestamp", timestamp)
        .header("x-signature-ed25519", signature)
        .body(Body::from(body))
        .expect("request");

    let response = app.oneshot(request).await.expect("router response");

    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };

    Response {
        status,
        headers,
        body,
    }
}

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn help(pool: PgPool) {
    let response = interaction(
        router(pool),
        execute_from_guild(json!({ "name": "help" }), 12),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let description = format!(
        "VirtualCryptoはDiscord上でサーバーに独自の通貨を作成できるBotです。\n\
         [コマンドの使い方の詳細]({SITE_URL}/document/commands)\n\
         [公式サイト]({SITE_URL})\n\
         [Botの招待]({BOT_INVITE_URL})\n\
         [サポートサーバーの招待]({SUPPORT_GUILD_INVITE_URL})"
    );

    assert_eq!(
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "color": 0x0062_21ED,
                    "title": "VirtualCrypto",
                    "thumbnail": { "url": "https://vcrypto.sumidora.com/static/images/logo.jpg" },
                    "description": description,
                }],
            },
        })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn invite(pool: PgPool) {
    let response = interaction(
        router(pool),
        execute_from_guild(json!({ "name": "invite" }), 12),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let description = format!(
        "[Botの招待]({BOT_INVITE_URL})\n[サポートサーバーの招待]({SUPPORT_GUILD_INVITE_URL})"
    );

    assert_eq!(
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "color": 0x0062_21ED,
                    "title": "VirtualCrypto",
                    "thumbnail": { "url": "https://vcrypto.sumidora.com/static/images/logo.jpg" },
                    "description": description,
                }],
            },
        })
    );
}

/// `verified/2` needs `data.name`; without it the interaction falls through to
/// the same `Type Not Found` an unhandled type gets.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_command_without_a_name_is_400(pool: PgPool) {
    let response = interaction(router(pool), execute_from_guild(json!({}), 12)).await;

    assert_eq!(response.status, 400);
    assert_eq!(response.body, Value::String("Type Not Found".into()));
}

/// The commands that are still to come answer 501 until they land; see
/// docs/test-port.md for the list.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unimplemented_command_is_501(pool: PgPool) {
    let response = interaction(
        router(pool),
        execute_from_guild(json!({ "name": "bal" }), 12),
    )
    .await;

    assert_eq!(response.status, 501);
}
