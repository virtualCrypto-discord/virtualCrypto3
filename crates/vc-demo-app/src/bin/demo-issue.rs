//! A sample that issues from a guild's pool, two ways in.
//!
//! The one program exercises both halves of the issuing contract: the permission
//! and the spend. What differs between the two ways is the permission, and that is
//! the whole of the difference the flows below have to show.
//!
//! - Web, on the application's own page: the operator opens
//!   `/applications/<client_id>` in the SPA, enters the guild's id, and the page
//!   posts it to `/applications/<client_id>/grants`. The permission the guild's
//!   administrator gives is the one this program then spends.
//! - Discord, in the guild: the operator runs `/grant allow <client_id>` and
//!   presses **許可する**, or the application asks first with
//!   `POST /oauth2/clients/@me/grant-requests` and the operator answers in
//!   `/grant list`. Same grant either way.
//!
//! ```text
//! cargo run -p vc-demo-app --bin demo-issue -- \
//!     --service http://127.0.0.1:4000 \
//!     --client-id 00000000-0000-0000-0000-000000000000 \
//!     --client-secret the-secret \
//!     --guild-id 900000000000000001 \
//!     --receiver-id 100000000000000002 \
//!     --amount 100
//! ```
//!
//! The program assumes the grant is already there — from the page, from the
//! command, or from a request the guild answered — and says exactly that when it
//! is not. It is the spend that is exercised here, not the asking: what that
//! refusal carries (`invalid_client` for a guild with no grant) is the thing that
//! tells an operator which of the two ways in above they still have to take.
//!
//! No webhook, and deliberately none: this flow delivers no event, so there is
//! nothing to verify, and the application's public key is not needed. What stays
//! of the application's credentials is the confidential part — the secret never
//! leaves this process except in the Basic header the token endpoint requires.

use serde::Deserialize;

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
         --guild-id <snowflake> --receiver-id <snowflake> --amount <int>"
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
    let guild_id = argument("--guild-id");
    let receiver_id = argument("--receiver-id");
    let amount = argument("--amount");

    let client = reqwest::Client::new();

    // `client_credentials` with a guild id: the token is the grant's own id, and
    // the guild it was issued for is the pool this program may spend. The secret
    // travels in the Basic header, which is the one part of this flow that only a
    // confidential client can do.
    let token: Token = client
        .post(format!("{service}/oauth2/token"))
        .basic_auth(&client_id, Some(&client_secret))
        .form(&[
            ("grant_type", "client_credentials"),
            ("guild_id", guild_id.as_str()),
        ])
        .send()
        .await
        .expect("the token endpoint answers")
        .error_for_status()
        .unwrap_or_else(|error| {
            eprintln!(
                "no guild token — the grant is not there yet: {error}\n\
                 web:  open /applications/{client_id} in the SPA, \
                 enter the guild id, and allow it\n\
                 discord: /grant allow {client_id}, then press 許可する\n\
                 discord, asked first: POST /oauth2/clients/@me/grant-requests, \
                 then answer in /grant list"
            );
            std::process::exit(1)
        })
        .json()
        .await
        .expect("a guild token");

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
