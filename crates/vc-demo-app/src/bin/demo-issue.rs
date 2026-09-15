//! A sample that issues from a guild's pool, the device-flow way in.
//!
//! The one program exercises both halves of the issuing contract: the ask and
//! the spend. The permission always starts with the application asking —
//! `POST /oauth2/clients/@me/grant-requests` with the scopes it wants — and
//! ends with the guild answering in Discord:
//!
//! - Web, on the application's own page: the operator opens
//!   `/applications/<client_id>` in the SPA, enters the guild's id, and the page
//!   posts it to `/applications/<client_id>/grants`. The permission the guild's
//!   administrator gives is the one this program then spends. This is the owner's
//!   own shortcut, and the only write that skips the ask.
//! - Discord, in the guild: the application asks first, prints the `user_code`
//!   it was answered with, and the operator runs `/grant approve code:<user_code>`.
//!
//! ```text
//! cargo run -p vc-demo-app --bin demo-issue -- \
//!     --service http://127.0.0.1:4000 \
//!     --client-id 00000000-0000-0000-0000-000000000000 \
//!     --client-secret the-secret \
//!     --registration-token the-registration-token \
//!     --guild-id 900000000000000001 \
//!     --receiver-id 100000000000000002 \
//!     --amount 100
//! ```
//!
//! The ask comes first: without an approved grant there is nothing to spend,
//! and the program says exactly that — the `user_code` to approve, and the
//! command to approve it with. Then it polls the token endpoint the way RFC 8628
//! says a device polls, and spends the guild token the poll answers with.
//!
//! No webhook, and deliberately none: this flow delivers no event, so there is
//! nothing to verify, and the application's public key is not needed. What stays
//! of the application's credentials is the confidential part — the secret never
//! leaves this process except in the Basic header the token endpoint requires.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Ask {
    device_code: String,
    user_code: String,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct Token {
    access_token: String,
    token_type: String,
}

#[derive(Debug, Deserialize)]
struct Issued {
    amount: String,
    pool_amount: String,
    unit: String,
}

fn usage() -> ! {
    eprintln!(
        "usage: demo-issue --service <url> --client-id <uuid> --client-secret <secret> \
         --registration-token <token> --guild-id <snowflake> --receiver-id <snowflake> --amount <int>"
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let argument = |name: &str| -> String {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|at| arguments.get(at + 1))
            .cloned()
            .unwrap_or_else(|| usage())
    };

    let service = argument("--service").trim_end_matches('/').to_owned();
    let client_id = argument("--client-id");
    let client_secret = argument("--client-secret");
    let registration_token = argument("--registration-token");
    let guild_id = argument("--guild-id");
    let receiver_id = argument("--receiver-id");
    let amount = argument("--amount");

    let client = reqwest::Client::new();

    // The ask: what this application wants, named so the approval can answer
    // exactly it. The `device_code` is what the poll below names; the
    // `user_code` is what the guild's administrator types.
    let ask: Ask = client
        .post(format!("{service}/oauth2/clients/@me/grant-requests"))
        .bearer_auth(&registration_token)
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "guild_id": guild_id,
            "scopes": ["vc.issue"],
        }))
        .send()
        .await
        .expect("the grant-request endpoint answers")
        .error_for_status()
        .unwrap_or_else(|error| {
            eprintln!("the guild could not be asked: {error}");
            std::process::exit(1)
        })
        .json()
        .await
        .expect("an ask");

    println!(
        "asked; approve it with `/grant approve code:{}` in guild {guild_id} (expires in {}s)",
        ask.user_code, ask.expires_in
    );

    // The poll, the way RFC 8628 says a device polls: the same `device_code`
    // until the guild answers, and `authorization_pending` is the answer that
    // means "not yet".
    let token: Token = loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let answer = client
            .post(format!("{service}/oauth2/token"))
            .basic_auth(&client_id, Some(&client_secret))
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", ask.device_code.as_str()),
            ])
            .send()
            .await
            .expect("the token endpoint answers");

        if answer.status() == reqwest::StatusCode::OK {
            break answer.json().await.expect("a guild token");
        }

        let body: serde_json::Value = answer.json().await.unwrap_or_default();

        if body["error"] != "authorization_pending" {
            eprintln!("the guild will not issue: {body}");
            std::process::exit(1)
        }
    };

    assert_eq!(
        token.token_type, "Bearer",
        "the endpoint answers Bearer and nothing else"
    );

    // The spend. The guild is the token's, so the path names none: this is the
    // request a guild token — and nothing else — is for.
    let issued: Issued = client
        .post(format!("{service}/api/v2/currencies/issue"))
        .bearer_auth(&token.access_token)
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "receiver_discord_id": receiver_id,
            "amount": amount,
        }))
        .send()
        .await
        .expect("the issuing endpoint answers")
        .error_for_status()
        .unwrap_or_else(|error| {
            eprintln!("the guild would not issue: {error}");
            std::process::exit(1)
        })
        .json()
        .await
        .expect("an issuance");

    println!(
        "issued {} {} to {receiver_id}; the pool holds {}",
        issued.amount, issued.unit, issued.pool_amount
    );
}
