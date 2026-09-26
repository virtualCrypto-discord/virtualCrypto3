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
//! Rate-limited requests wait for `Retry-After` and retry the same request,
//! including the same charge key. Statements are read in pages of 200 rows;
//! only charge events count towards the billed total, not locks or refunds.
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
    id: String,
    event: String,
    amount: String,
}

#[derive(Default)]
struct Statement {
    charges: usize,
    billed: i128,
}

const STATEMENT_PAGE_SIZE: usize = 200;

/// A 429 is refused before the API writes anything. Clone the complete request
/// so a charge keeps its original body and idempotency key while it waits.
async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, reqwest::Error> {
    loop {
        let response = request
            .try_clone()
            .expect("demo requests have replayable JSON or form bodies")
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Ok(response);
        }

        // The service sends a whole number of seconds. A missing or malformed
        // header falls back to waiting past its normal one-minute window.
        let seconds = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(61)
            .max(1);
        drop(response);
        eprintln!("rate limited; waiting {seconds}s");
        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
    }
}

async fn read_statement(
    client: &reqwest::Client,
    service: &str,
    token: &str,
    contract: &str,
) -> Result<Statement, Box<dyn std::error::Error>> {
    let mut statement = Statement::default();
    let mut next: Option<String> = None;
    loop {
        let mut request = client
            .get(format!("{service}/api/v2/contracts/{contract}/payments"))
            .bearer_auth(token)
            .header("Accept", "application/json")
            .query(&[("limit", STATEMENT_PAGE_SIZE.to_string())]);
        if let Some(id) = &next {
            request = request.query(&[("next", id)]);
        }
        let entries: Vec<Entry> = send(request).await?.error_for_status()?.json().await?;
        let more = entries.len() == STATEMENT_PAGE_SIZE;
        next = entries.last().map(|entry| entry.id.clone());
        for entry in entries.into_iter().filter(|entry| entry.event == "charge") {
            statement.charges += 1;
            statement.billed += i128::from(entry.amount.parse::<i64>()?);
        }
        if !more {
            return Ok(statement);
        }
    }
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
    let created: Contract = send(
        client
            .post(format!("{service}/api/v2/contracts"))
            .bearer_auth(&token.access_token)
            .header("Accept", "application/json")
            .json(&serde_json::json!({
                "unit": unit,
                "parties": [{ "discord_id": subscriber, "amount": quota }],
                "receiver_discord_id": receiver,
                "expires_in": period,
            })),
    )
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

        let contract: Contract = send(
            client
                .get(format!("{service}/api/v2/contracts/{}", created.id))
                .bearer_auth(&token.access_token)
                .header("Accept", "application/json"),
        )
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
    let mut completed = 0;

    for use_number in 1..=uses {
        println!("use {use_number}: doing the work");

        let answer = send(
            client
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
                })),
        )
        .await
        .expect("the payment endpoint answers");

        // The status is read before `error_for_status`, which would consume the
        // answer: a refusal says which refusal it is, and that is in the body.
        if answer.status() == reqwest::StatusCode::CREATED {
            let charged: Charged = answer.json().await.expect("a charge");
            left = charged.remaining.clone();
            completed += 1;

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
                break;
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
        println!("{completed} uses done: the quota is spent");
    } else {
        println!(
            "{completed} uses done; {left} {unit} is still locked and goes home when the period ends \
             — an application cannot end a contract early"
        );
    }

    // The complete ledger includes the initial lock and any refunds. Read all
    // pages and count only charges, including when the quota ended the run.
    let statement = read_statement(&client, &service, &token.access_token, &created.id)
        .await
        .unwrap_or_else(|error| {
            eprintln!("the statement could not be read: {error}");
            std::process::exit(1)
        });

    println!(
        "the statement has {} {}, {} {unit} billed in total",
        statement.charges,
        if statement.charges == 1 {
            "entry"
        } else {
            "entries"
        },
        statement.billed,
    );
}
