//! Check the browser link and webhook handshake against the running demo.

use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use std::{
    collections::HashMap,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const GUILD: &str = "100000000000000001";
const CLIENT: &str = "00000000-0000-0000-0000-000000000001";

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn demo() -> (Running, String, reqwest::Client) {
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port().to_string();
    drop(socket);
    let key = SigningKey::from_bytes(&[7; 32]);
    let public: String = key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let child = Running(
        Command::new(env!("CARGO_BIN_EXE_vc-demo-app"))
            .args([
                "--service",
                "http://127.0.0.1:12345",
                "--client-id",
                CLIENT,
                "--client-secret",
                "test-secret",
                "--guild-id",
                GUILD,
                "--public-key",
                &public,
                "--port",
                &port,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if http.get(&base).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    (child, base, http)
}

#[tokio::test]
async fn authorization_link_requests_issuance_in_the_configured_guild() {
    let (_child, base, http) = demo().await;
    let response = http.get(&base).send().await.unwrap();
    assert_eq!(
        response.headers()["content-type"],
        "text/html; charset=utf-8"
    );
    let body = response.text().await.unwrap();
    let href = body
        .split("href=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .replace("&amp;", "&");
    let url = reqwest::Url::parse(&href).unwrap();
    assert_eq!(url.origin().ascii_serialization(), "http://127.0.0.1:12345");
    assert_eq!(url.path(), "/oauth2/authorize");
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["guild_id"], GUILD);
    assert_eq!(query["scope"], "vc.issue");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["client_id"], CLIENT);
    assert_eq!(query["redirect_uri"], format!("{base}/callback"));
}

#[tokio::test]
async fn webhook_ping_returns_pong_only_for_the_registered_key() {
    let (_child, base, http) = demo().await;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    for (seed, event) in [(7, 1), (8, 1), (7, 2)] {
        let body = json!({"type":event}).to_string();
        let key = SigningKey::from_bytes(&[seed; 32]);
        let signature: String = key
            .sign(format!("{timestamp}{body}").as_bytes())
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let response = http
            .post(format!("{base}/webhook"))
            .header("X-Signature-Timestamp", &timestamp)
            .header("X-Signature-Ed25519", signature)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        if seed == 7 {
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            if event == 1 {
                assert_eq!(response.headers()["content-type"], "application/json");
                assert_eq!(
                    response.json::<serde_json::Value>().await.unwrap(),
                    json!({"type":1})
                );
            }
        } else {
            assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        }
    }
}
