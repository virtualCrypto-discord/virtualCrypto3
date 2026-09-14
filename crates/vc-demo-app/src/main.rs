//! A small application, for exercising the flow this service is for.
//!
//! It is not part of the service; it is the other side of it. Run it, open the address it prints,
//! and the three things an application does happen in order:
//!
//! - the authorization code comes back to `/callback`,
//! - it is exchanged at `/oauth2/token`,
//! - and a webhook that arrives at `/webhook` is verified with the application's public key, which
//!   is the check the service's handshake is asking for. A webhook that does not verify is refused
//!   with `401`, which is what makes a handshake pass.
//!
//! ```text
//! cargo run -p vc-demo-app -- \
//!     --service http://127.0.0.1:4000 \
//!     --client-id 00000000-0000-0000-0000-000000000000 \
//!     --client-secret the-secret \
//!     --public-key the-hex-the-application-reads
//! ```
//!
//! The public key is the one `GET /oauth2/clients/@me` answers with, lowercased hex. Nothing here
//! is a secret: the private half never leaves the service, which is what the signature is for.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;

/// What the operator typed, and the key webhooks are signed with.
struct Application {
    service: String,
    client_id: String,
    client_secret: String,
    public: VerifyingKey,
    /// Where this application listens, which is also the redirect URI it must have registered.
    address: String,
}

/// The one parameter the callback carries.
#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    error_description: Option<String>,
}

#[tokio::main]
async fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let argument = |name: &str| -> Option<String> {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|at| arguments.get(at + 1))
            .cloned()
    };

    let port = argument("--port").unwrap_or_else(|| "8080".to_owned());
    let service = argument("--service").expect("--service is the address of the service");
    let client_id = argument("--client-id").expect("--client-id is what was registered");
    let client_secret = argument("--client-secret").expect("--client-secret is what was issued");
    let public =
        argument("--public-key").expect("--public-key is the hex the service answers with");

    let bytes: Vec<u8> = (0..public.len() / 2)
        .filter_map(|pair| u8::from_str_radix(&public[pair * 2..pair * 2 + 2], 16).ok())
        .collect();

    let public = VerifyingKey::from_bytes(&bytes.try_into().expect("32 bytes of hex"))
        .expect("a public key");

    let address = format!("127.0.0.1:{port}");

    let application = Arc::new(Application {
        service: service.trim_end_matches('/').to_owned(),
        client_id,
        client_secret,
        public,
        address: address.clone(),
    });

    let app = Router::new()
        .route("/", get(home))
        .route("/callback", get(callback))
        .route("/webhook", post(webhook))
        .with_state(application);

    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("the port this application listens on");

    println!("listening on http://{address}");
    println!("open it, and the flow starts");

    axum::serve(listener, app).await.expect("the server");
}

/// The page a person opens: the address the service authorizes at, with this application's own
/// callback as the place the code comes back to.
async fn home(State(application): State<Arc<Application>>) -> impl IntoResponse {
    let authorize = format!(
        "{}/oauth2/authorize?response_type=code&client_id={}&redirect_uri=http://{}/callback",
        application.service, application.client_id, application.address
    );

    format!("<a href=\"{authorize}\">{authorize}</a>")
}

/// Where the code comes back. It is exchanged at the token endpoint with the application's own
/// credentials, which is the part that only a confidential client can do.
async fn callback(
    State(application): State<Arc<Application>>,
    Query(callback): Query<Callback>,
) -> impl IntoResponse {
    if let Some(description) = callback.error_description {
        return format!("the service refused: {description}");
    }

    let Some(code) = callback.code else {
        return "the callback carried no code".to_owned();
    };

    let form = [
        ("grant_type", "authorization_code".to_owned()),
        ("code", code),
        (
            "redirect_uri",
            format!("http://{}/callback", application.address),
        ),
        ("client_id", application.client_id.clone()),
        ("client_secret", application.client_secret.clone()),
    ];

    let answer = reqwest::Client::new()
        .post(format!("{}/oauth2/token", application.service))
        .form(&form)
        .send()
        .await;

    match answer {
        Ok(answer) => format!("the token endpoint answered: {:?}", answer.text().await),
        Err(error) => format!("the token endpoint could not be reached: {error}"),
    }
}

/// A webhook: verified the way the service's handshake expects, which is a PING it signs with the
/// key this application holds the public half of.
async fn webhook(
    State(application): State<Arc<Application>>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    };

    let signature: Option<Vec<u8>> = (0..header("X-Signature-Ed25519").len() / 2)
        .map(|pair| {
            u8::from_str_radix(&header("X-Signature-Ed25519")[pair * 2..pair * 2 + 2], 16).ok()
        })
        .collect();

    let verified = signature
        .and_then(|bytes| <[u8; 64]>::try_from(bytes).ok())
        .map(|bytes| Signature::from_bytes(&bytes))
        .is_some_and(|signature| {
            application
                .public
                .verify(
                    format!("{}{body}", header("X-Signature-Timestamp")).as_bytes(),
                    &signature,
                )
                .is_ok()
        });

    if verified {
        println!("a webhook verified: {body}");

        (StatusCode::OK, "verified".to_owned())
    } else {
        // What the service is asking for: a signature it did not make is a signature this
        // application refuses.
        println!("a webhook did not verify: {body}");

        (StatusCode::UNAUTHORIZED, "not verified".to_owned())
    }
}
