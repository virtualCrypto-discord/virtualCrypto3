//! A sample that bills by use: a quota locked under a contract, drawn down one
//! use at a time.
//!
//! This is the shape a pay-per-use application has. The subscriber pre-pays in
//! the only way this service lets anyone pre-pay — they lock an amount by
//! approving a contract, and from that moment the application may spend it. Each
//! use charges one price, and the run ends where a pre-paid quota should end:
//! with nothing left to draw on and nothing owed.
//!
//! - **The application asks.** `POST /api/v2/contracts` names the subscriber,
//!   what they lock, how long the delegation runs, and — fixed at creation — the
//!   receiver every charge goes to. Fixing the receiver is what the subscriber
//!   is trusting: the money can reach that one account and no other, whatever
//!   the application does next.
//! - **The subscriber answers.** The contract is in `/contract list` in Discord,
//!   and approving it is what locks the amount. Nothing is locked before that,
//!   and no guild is asked anything: issuing is a guild's decision about its
//!   pool, while a contract is one user deciding to let one application spend
//!   their money. That is the difference between this program and `demo-issue`.
//! - **The period is what makes it a subscription.** A contract with
//!   `expires_in` cannot be withdrawn from while it runs, and at the deadline
//!   whatever is left goes home on its own — which is also why the application
//!   is told, when the quota runs out early, that it cannot close the period
//!   itself.
//!
//! ```text
//! cargo run -p vc-demo-app --bin demo-billing -- \
//!     --service http://127.0.0.1:4000 \
//!     --client-id 00000000-0000-0000-0000-000000000000 \
//!     --client-secret the-secret \
//!     --discord-id 100000000000000001 \
//!     --unit nyan \
//!     --quota 300 \
//!     --price 25 \
//!     --uses 20 \
//!     --receiver-id 500000000000000001
//! ```
//!
//! The credentials are the application's own, and the token they buy is an
//! application token: `grant_type=client_credentials` with `scope=vc.contract`,
//! which is the scope that says an application may *ask*. What lets it spend is
//! the subscriber's approval, not the scope.
//!
//! Two things it does that a demo does not have to and an application does.
//! **Every charge carries an `Idempotency-Key`** — the contract's id and the use
//! it is for — so a charge whose answer was lost is retried rather than repeated;
//! the key is what makes a metered bill reconcilable. And **the statement is read
//! back at the end** (`GET /api/v2/contracts/{id}/payments`), because a charge
//! that cannot be read again is not a receipt.
//!
//! No webhook, and deliberately none: the approval is observed by reading the
//! contract until it is `active`, so there is nothing to verify and the
//! application's public key is not needed here. A real application would keep
//! its webhook — the type-4 event is this same decision, pushed rather than
//! polled.

use serde::Deserialize;

/// The application token, which is the only thing the token endpoint answers
/// here.
#[derive(Debug, Deserialize)]
struct Token {
    access_token: String,
    token_type: String,
}

/// As much of a contract as this program reads: what to name, and whether the
/// subscriber has answered.
#[derive(Debug, Deserialize)]
struct Contract {
    id: String,
    status: String,
}

/// What a charge moved, and what is left to move.
#[derive(Debug, Deserialize)]
struct Charged {
    amount: String,
    remaining: String,
    unit: Option<String>,
}

/// One line of the statement: what a slice of a charge came to. The rest of the
/// row — the party it came out of, the receiver, the time — is for a reader that
/// shows it rather than sums it.
#[derive(Debug, Deserialize)]
struct Entry {
    amount: String,
}

/// A period, in seconds: thirty days, which is the shape of a month's quota.
const MONTH: i64 = 30 * 24 * 60 * 60;

/// How often the contract is read while waiting for the approval.
const POLL: std::time::Duration = std::time::Duration::from_secs(5);

fn usage() -> ! {
    eprintln!(
        "usage: demo-billing --service <url> --client-id <uuid> --client-secret <secret> \
         --discord-id <snowflake> --unit <unit> --quota <int> --price <int> --uses <int> \
         --receiver-id <snowflake> [--period <seconds>]"
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
    let optional = |name: &str, absent: &str| -> String {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|at| arguments.get(at + 1))
            .cloned()
            .unwrap_or_else(|| absent.to_owned())
    };

    let service = argument("--service").trim_end_matches('/').to_owned();
    let client_id = argument("--client-id");
    let client_secret = argument("--client-secret");
    let subscriber = argument("--discord-id");
    let unit = argument("--unit");
    let quota = argument("--quota");
    let price = argument("--price");
    let uses: i64 = argument("--uses").parse().unwrap_or_else(|_| usage());
    let receiver = argument("--receiver-id");
    let period: i64 = optional("--period", &MONTH.to_string())
        .parse()
        .unwrap_or_else(|_| usage());

    let client = reqwest::Client::new();

    // The application signing in as itself. The scope is what it wants to be
    // able to ask for; the parties' approvals are what it will be able to spend.
    let token: Token = client
        .post(format!("{service}/oauth2/token"))
        .basic_auth(&client_id, Some(&client_secret))
        .form(&[
            ("grant_type", "client_credentials"),
            ("scope", "vc.contract"),
        ])
        .send()
        .await
        .expect("the token endpoint answers")
        .error_for_status()
        .unwrap_or_else(|error| {
            eprintln!("the application could not sign in: {error}");
            std::process::exit(1)
        })
        .json()
        .await
        .expect("an application token");

    assert_eq!(
        token.token_type, "Bearer",
        "the endpoint answers Bearer and nothing else"
    );

    // The ask. Ids and amounts travel as strings — JSON's number cannot hold a
    // snowflake exactly — and the period is a number of seconds.
    let created: Contract = client
        .post(format!("{service}/api/v2/contracts"))
        .bearer_auth(&token.access_token)
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "unit": unit,
            "parties": [{ "discord_id": subscriber, "amount": quota }],
            "receiver_discord_id": receiver,
            "expires_in": period,
        }))
        .send()
        .await
        .expect("the contract endpoint answers")
        .error_for_status()
        .unwrap_or_else(|error| {
            eprintln!("the contract could not be asked for: {error}");
            std::process::exit(1)
        })
        .json()
        .await
        .expect("a contract");

    println!(
        "asked; approve it with `/contract list` in Discord (contract {}, {quota} {unit} on approval)",
        created.id
    );

    // The approval, which is the subscriber's and arrives whenever they get to
    // it: the contract is read until they have answered. One party is one
    // approval, so the first one that lands is also the last.
    loop {
        tokio::time::sleep(POLL).await;

        let contract: Contract = client
            .get(format!("{service}/api/v2/contracts/{}", created.id))
            .bearer_auth(&token.access_token)
            .header("Accept", "application/json")
            .send()
            .await
            .expect("the contract endpoint answers")
            .error_for_status()
            .unwrap_or_else(|error| {
                eprintln!("the contract could not be read: {error}");
                std::process::exit(1)
            })
            .json()
            .await
            .expect("a contract");

        match contract.status.as_str() {
            "pending" => continue,
            "active" => break,
            decided => {
                eprintln!("the contract is {decided}: nothing will be billed");
                std::process::exit(1)
            }
        }
    }

    println!("active: {quota} {unit} locked for {period}s, and every charge goes to {receiver}");

    // The meter. Each use does its work and then charges one price for it — a
    // partial spend of the same approval, which is what makes this billing by
    // use rather than a single up-front payment.
    let mut left = quota.clone();

    for use_number in 1..=uses {
        println!("use {use_number}: doing the work");

        let answer = client
            .post(format!(
                "{service}/api/v2/contracts/{}/payments",
                created.id
            ))
            .bearer_auth(&token.access_token)
            .header("Accept", "application/json")
            // The contract and the use, which is a key no other charge of this
            // run (or of the next one, whose contract is new) shares. The quotes
            // are part of it: the header is a quoted string, and one that is not
            // is refused before anything is charged.
            .header(
                "Idempotency-Key",
                format!("\"{}-{use_number}\"", created.id),
            )
            .json(&serde_json::json!({
                "receiver_discord_id": receiver,
                "amount": price,
            }))
            .send()
            .await
            .expect("the payment endpoint answers");

        // The status is read before `error_for_status`, which would consume the
        // answer: a refusal says which refusal it is, and that is in the body.
        if answer.status() == reqwest::StatusCode::CREATED {
            let charged: Charged = answer.json().await.expect("a charge");
            left = charged.remaining.clone();

            println!(
                "use {use_number}: charged {} {}, {} left",
                charged.amount,
                charged.unit.as_deref().unwrap_or(unit.as_str()),
                charged.remaining
            );
            continue;
        }

        let refused: serde_json::Value = answer.json().await.unwrap_or_default();

        match refused["error_info"].as_str() {
            // A prepaid quota running out is the end it was written to have, not
            // a failure: the work stops, and nothing is owed.
            Some("not_enough_amount") => {
                println!("the quota is gone: work stops, and nothing more is owed");
                return;
            }
            Some("expired") => {
                eprintln!("the period is over: nothing may be spent after it");
                std::process::exit(1)
            }
            _ => {
                eprintln!("the charge was refused: {refused}");
                std::process::exit(1)
            }
        }
    }

    if left == "0" {
        println!("{uses} uses done: the quota is spent");
    } else {
        println!(
            "{uses} uses done; {left} {unit} is still locked and goes home when the period ends \
             — an application cannot end a contract early"
        );
    }

    // The receipt, read back the way the subscriber can read it too. One charge
    // is one row per party it drew on, and this contract names one person, so the
    // rows are the charges.
    let statement: Vec<Entry> = client
        .get(format!(
            "{service}/api/v2/contracts/{}/payments?limit={uses}",
            created.id
        ))
        .bearer_auth(&token.access_token)
        .header("Accept", "application/json")
        .send()
        .await
        .expect("the statement endpoint answers")
        .error_for_status()
        .unwrap_or_else(|error| {
            eprintln!("the statement could not be read: {error}");
            std::process::exit(1)
        })
        .json()
        .await
        .expect("a statement");

    let billed: i64 = statement
        .iter()
        .map(|entry| entry.amount.parse::<i64>().unwrap_or_default())
        .sum();

    println!(
        "the statement has {} {} of them, {billed} {unit} in total",
        statement.len(),
        if statement.len() == 1 {
            "entry"
        } else {
            "entries"
        }
    );
}
