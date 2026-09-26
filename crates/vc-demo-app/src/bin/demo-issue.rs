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
//!
//! ## Distributing to a role
//!
//! `--role-id` in place of `--receiver-id` pays everyone who holds that role,
//! `--amount` each. The sample reads Discord itself with `--bot-token`, so the
//! service is asked to do nothing it did not already do.
//! Register `refresh_token` in the application's `grant_types` for role runs.
//! The program renews expired tokens and continues with the unpaid member.
//!
//! ```text
//! cargo run -p vc-demo-app --bin demo-issue -- \
//!     --service http://127.0.0.1:4000 \
//!     --client-id 00000000-0000-0000-0000-000000000000 \
//!     --client-secret the-secret \
//!     --registration-token the-registration-token \
//!     --guild-id 900000000000000001 \
//!     --role-id 900000000000000009 \
//!     --bot-token the-bot-token \
//!     --amount 100
//! ```
//!
//! Every refusal that needs no person is settled **before** the device flow:
//! the guild's size, that the role exists, who holds it, and whether the pool
//! pays for all of them. Nobody is asked to approve a run that cannot happen.
//!
//! **The size is Discord's own number.** [Gateway Events][gateway] says it twice,
//! under *Guild Create* and under *Request Guild Members*:
//!
//! > if the guild has over 75k members, it will only send members who are in
//! > voice, plus the member for you (the connecting user)
//!
//! REST's [List Guild Members][list] states no size limit of its own — only that
//! it requires the `GUILD_MEMBERS` privileged intent and that `limit` is 1-1000
//! per page — so 75,000 is the one figure Discord writes down for member
//! enumeration failing at scale, and it is what this refuses past. The bot needs
//! that intent enabled in the Developer Portal and needs to be a member of
//! `--guild-id`; both refusals arrive from Discord as a non-200 and are reported
//! with the body that came with it.
//!
//! `@everyone`'s id is never written into a member's `roles`, so passing it
//! finds nobody and is refused as a role with no holders.
//!
//! [gateway]: https://docs.discord.com/developers/events/gateway-events
//! [list]: https://docs.discord.com/developers/resources/guild#list-guild-members

use serde::Deserialize;
use serde_json::Value;

/// The size at which Discord stops handing over a member list, so the size this
/// sample refuses past.
///
/// From [Gateway Events][gateway], under *Guild Create* and *Request Guild
/// Members*: past this the members Discord sends are "only … your bot and users
/// in voice channels". `over 75k` is strict, so a guild of exactly 75,000 is
/// still enumerated.
///
/// REST's own *List Guild Members* carries no size limit — only the
/// `GUILD_MEMBERS` privileged intent and `limit` 1-1000 — which is why the
/// gateway's figure is the one used here.
///
/// [gateway]: https://docs.discord.com/developers/events/gateway-events
const MAX_GUILD_MEMBERS: i64 = 75_000;

/// One page of `GET /guilds/{id}/members`, which Discord caps at 1000.
const MEMBER_PAGE: usize = 1000;

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
    refresh_token: Option<String>,
}

impl Token {
    async fn renew(&mut self, client: &reqwest::Client, service: &str) -> Result<(), String> {
        let refresh_token = self.refresh_token.as_deref().ok_or_else(|| {
            "token renewal requires refresh_token in the application's grant_types".to_owned()
        })?;
        let answer = client
            .post(format!("{service}/oauth2/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await
            .map_err(|error| format!("the token endpoint could not be reached: {error}"))?;
        if answer.status() != reqwest::StatusCode::OK {
            let refused: Value = answer.json().await.unwrap_or(Value::Null);
            return Err(format!("the guild token could not be renewed: {refused}"));
        }
        let next: Token = answer
            .json()
            .await
            .map_err(|error| format!("the token endpoint returned an invalid token: {error}"))?;
        if next.token_type != "Bearer" || next.refresh_token.is_none() {
            return Err("the token endpoint did not return a renewable Bearer token".into());
        }
        // Refresh tokens rotate: the next renewal must use the replacement.
        *self = next;
        Ok(())
    }
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
         --registration-token <token> --guild-id <snowflake> --amount <int> \
         (--receiver-id <snowflake> | --role-id <snowflake> --bot-token <token>)"
    );
    std::process::exit(2)
}

/// Past the size Discord stops listing members for.
fn too_large(member_count: i64) -> bool {
    member_count > MAX_GUILD_MEMBERS
}

/// The ids of the members carrying `role_id`, in the order Discord sent them.
fn holders_of(members: &[Value], role_id: &str) -> Vec<String> {
    members
        .iter()
        .filter_map(|member| {
            let carries = member
                .get("roles")
                .and_then(Value::as_array)?
                .iter()
                .any(|role| role.as_str() == Some(role_id));
            if !carries {
                return None;
            }
            Some(member.get("user")?.get("id")?.as_str()?.to_owned())
        })
        .collect()
}

/// What a run pays out, or `None` when the multiplication leaves an `i64`.
fn total_for(holders: usize, amount: i64) -> Option<i64> {
    i64::try_from(holders).ok()?.checked_mul(amount)
}

/// One Discord GET that has to succeed, or the refusal it got instead.
///
/// The body travels with a non-200 because that is where Discord says which
/// refusal it is — a bad bot token, a bot that is not in the guild, or the
/// `GUILD_MEMBERS` intent the member list needs.
async fn discord_read(
    client: &reqwest::Client,
    api: &str,
    bot_token: &str,
    what: &str,
    path: &str,
) -> Value {
    let answer = client
        .get(format!("{api}{path}"))
        .header("Authorization", format!("Bot {bot_token}"))
        .send()
        .await
        .unwrap_or_else(|error| {
            eprintln!("Discord could not be reached: {error}");
            std::process::exit(1)
        });

    let status = answer.status().as_u16();
    let body: Value = answer.json().await.unwrap_or(Value::Null);

    if status != 200 {
        eprintln!("Discord would not {what}: {status} {body}");
        std::process::exit(1);
    }

    body
}

/// The pool, from the one currency read that answers without a token.
async fn pool_for(client: &reqwest::Client, service: &str, guild_id: &str) -> i64 {
    let answer = client
        .get(format!("{service}/api/v2/currencies?guild={guild_id}"))
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap_or_else(|error| {
            eprintln!("the service could not be reached: {error}");
            std::process::exit(1)
        });

    let status = answer.status();
    let body: Value = answer.json().await.unwrap_or(Value::Null);

    if status != reqwest::StatusCode::OK {
        eprintln!("the service would not name guild {guild_id}'s currency: {body}");
        std::process::exit(1);
    }

    body.get("pool_amount")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            eprintln!("the service did not say what guild {guild_id}'s pool holds: {body}");
            std::process::exit(1)
        })
        .parse()
        // A null pool reaches the JSON as an empty string, and an empty pool is
        // what `issue_in` reads one as: zero.
        .unwrap_or(0)
}

/// Everything a role run can settle without a person, and the holders it is for.
///
/// Refusals exit from here rather than return: a run that cannot happen must not
/// reach the device flow, which is what asks an administrator to approve it.
async fn plan_role_run(
    client: &reqwest::Client,
    service: &str,
    api: &str,
    bot_token: &str,
    guild_id: &str,
    role_id: &str,
    amount: i64,
) -> Vec<String> {
    // The size, against Discord's own figure. A guild that does not answer with
    // one is left to the scan guard below, which counts what it walks.
    let guild = discord_read(
        client,
        api,
        bot_token,
        "read the guild",
        &format!("/guilds/{guild_id}?with_counts=true"),
    )
    .await;

    if let Some(member_count) = guild
        .get("approximate_member_count")
        .and_then(Value::as_i64)
        && too_large(member_count)
    {
        eprintln!(
            "guild {guild_id} has {member_count} members, past the {MAX_GUILD_MEMBERS} this \
             distributes to"
        );
        std::process::exit(1);
    }

    // The role belongs to this guild, so a mistyped id is this refusal rather
    // than the one below reading it as a role nobody holds.
    let roles = discord_read(
        client,
        api,
        bot_token,
        "read the guild's roles",
        &format!("/guilds/{guild_id}/roles"),
    )
    .await;

    let known = roles.as_array().is_some_and(|roles| {
        roles
            .iter()
            .any(|role| role.get("id").and_then(Value::as_str) == Some(role_id))
    });

    if !known {
        eprintln!("role {role_id} is not a role of guild {guild_id}");
        std::process::exit(1);
    }

    // Who holds it, a page at a time. The scan guard is what refuses when the
    // count above was missing or stale: a list walked past the limit is a
    // partial list, and issuing from one would pay some holders and not others.
    let mut holders: Vec<String> = Vec::new();
    let mut scanned: i64 = 0;
    let mut after: Option<String> = None;

    loop {
        let path = match &after {
            Some(cursor) => {
                format!("/guilds/{guild_id}/members?limit={MEMBER_PAGE}&after={cursor}")
            }
            None => format!("/guilds/{guild_id}/members?limit={MEMBER_PAGE}"),
        };

        let page = discord_read(
            client,
            api,
            bot_token,
            "list the guild's members (the GUILD_MEMBERS intent is required)",
            &path,
        )
        .await;

        let Some(page) = page.as_array() else {
            eprintln!("Discord answered the member list with something that is not a list");
            std::process::exit(1);
        };

        scanned = scanned.saturating_add(i64::try_from(page.len()).unwrap_or(i64::MAX));
        if too_large(scanned) {
            eprintln!(
                "walked past {MAX_GUILD_MEMBERS} members of guild {guild_id} without finishing; \
                 refusing rather than issuing from a partial list"
            );
            std::process::exit(1);
        }

        holders.extend(holders_of(page, role_id));

        if page.len() < MEMBER_PAGE {
            break;
        }

        // A full page means there is more, so the cursor has to be there —
        // stopping without one would hand out from a list that stopped early.
        let Some(cursor) = page
            .last()
            .and_then(|member| member.get("user"))
            .and_then(|user| user.get("id"))
            .and_then(Value::as_str)
            .map(|id| id.to_owned())
        else {
            eprintln!("Discord sent a full page of members with no id to page from");
            std::process::exit(1);
        };

        after = Some(cursor);
    }

    if holders.is_empty() {
        eprintln!("no member of guild {guild_id} holds role {role_id}");
        std::process::exit(1);
    }

    // The pool pays for all of it, or nobody is asked to approve a run that
    // would stop half way through paying.
    let total = total_for(holders.len(), amount).unwrap_or_else(|| {
        eprintln!(
            "{} members times {amount} does not fit in a number",
            holders.len()
        );
        std::process::exit(1)
    });
    let pool = pool_for(client, service, guild_id).await;

    if total > pool {
        eprintln!(
            "issuing {amount} to each of {} members of role {role_id} needs {total}, and guild \
             {guild_id}'s pool holds {pool}",
            holders.len()
        );
        std::process::exit(1);
    }

    holders
}

/// One issuance, waiting out the service's rate limit rather than giving up.
///
/// A 429 comes from the `GuildToken` extractor, which runs before the handler
/// does — nothing was written, so waiting and resending is safe without an
/// `Idempotency-Key`. The window is fixed, so sixty-one seconds is past its end.
/// A 401 also precedes the write. Renew and retry this member once; completed
/// members stay completed, and an unusable replacement must not loop forever.
async fn issue_to(
    client: &reqwest::Client,
    service: &str,
    token: &mut Token,
    receiver: &str,
    amount: &str,
) -> Result<Issued, String> {
    let mut renewed = false;
    loop {
        let answer = client
            .post(format!("{service}/api/v2/currencies/issue"))
            .bearer_auth(&token.access_token)
            .header("Accept", "application/json")
            .json(&serde_json::json!({
                "receiver_discord_id": receiver,
                "amount": amount,
            }))
            .send()
            .await
            .map_err(|error| format!("the issuing endpoint could not be reached: {error}"))?;

        if answer.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            eprintln!("rate limited; waiting 61s");
            tokio::time::sleep(std::time::Duration::from_secs(61)).await;
            continue;
        }

        if answer.status() == reqwest::StatusCode::UNAUTHORIZED && !renewed {
            token.renew(client, service).await?;
            renewed = true;
            continue;
        }

        if answer.status() != reqwest::StatusCode::CREATED {
            let refused: Value = answer.json().await.unwrap_or(Value::Null);
            return Err(format!(
                "the guild would not issue to {receiver}: {refused}"
            ));
        }

        return answer.json().await.map_err(|error| {
            format!("the issuing endpoint returned an invalid issuance: {error}")
        });
    }
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
    let optional = |name: &str| -> Option<String> {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|at| arguments.get(at + 1))
            .cloned()
    };

    let service = argument("--service").trim_end_matches('/').to_owned();
    let client_id = argument("--client-id");
    let client_secret = argument("--client-secret");
    let registration_token = argument("--registration-token");
    let guild_id = argument("--guild-id");
    let amount = argument("--amount");
    let receiver_id = optional("--receiver-id");
    let role_id = optional("--role-id");
    let bot_token = optional("--bot-token");
    let discord_api = match optional("--discord-api") {
        Some(api) => api.trim_end_matches('/').to_owned(),
        None => "https://discord.com/api".to_owned(),
    };

    // A role is read with the bot token, and one target is chosen rather than
    // two: both would be a run nobody can predict, and neither has nobody to pay.
    if role_id.is_some() && bot_token.is_none() {
        usage();
    }
    if receiver_id.is_some() == role_id.is_some() {
        usage();
    }

    let client = reqwest::Client::new();

    // Planned before the ask, never after it: the device flow is what asks an
    // administrator to approve, and approving a run that cannot happen is a
    // waste of theirs. Single-receiver mode plans nothing.
    let holders = if let (Some(role_id), Some(bot_token)) = (&role_id, &bot_token) {
        let amount = amount.parse::<i64>().unwrap_or_else(|_| usage());
        plan_role_run(
            &client,
            &service,
            &discord_api,
            bot_token,
            &guild_id,
            role_id,
            amount,
        )
        .await
    } else {
        Vec::new()
    };

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
    let mut token: Token = loop {
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
    //
    // Every holder of the role, one at a time. `holders` cannot be empty here:
    // an empty role was refused before the device flow, so this always runs.
    if let (Some(role_id), Some(_)) = (&role_id, &bot_token) {
        if token.refresh_token.is_none() {
            eprintln!("role distribution requires refresh_token in the application's grant_types");
            std::process::exit(1);
        }
        let mut pool_held = String::new();

        for (index, holder) in holders.iter().enumerate() {
            let issued = issue_to(&client, &service, &mut token, holder, &amount)
                .await
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(1)
                });
            pool_held = issued.pool_amount;

            println!(
                "issued {} {} to {holder} ({}/{})",
                issued.amount,
                issued.unit,
                index + 1,
                holders.len()
            );
        }

        println!(
            "issued {amount} to each of {} members of role {role_id}; the pool holds {pool_held}",
            holders.len()
        );

        return;
    }

    let receiver_id = receiver_id.expect("one target was chosen above");

    let issued = issue_to(&client, &service, &mut token, &receiver_id, &amount)
        .await
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(1)
        });

    println!(
        "issued {} {} to {receiver_id}; the pool holds {}",
        issued.amount, issued.unit, issued.pool_amount
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn seventy_five_thousand_is_still_enumerable_and_one_more_is_not() {
        assert!(!too_large(MAX_GUILD_MEMBERS));
        assert!(too_large(MAX_GUILD_MEMBERS + 1));
    }

    #[test]
    fn only_members_carrying_the_role_are_holders() {
        let members = vec![
            json!({ "user": { "id": "1" }, "roles": ["9"] }),
            json!({ "user": { "id": "2" }, "roles": ["7", "9"] }),
            json!({ "user": { "id": "3" }, "roles": [] }),
        ];

        assert_eq!(holders_of(&members, "9"), ["1".to_owned(), "2".to_owned()]);
        assert_eq!(holders_of(&members, "7"), ["2".to_owned()]);
        assert!(holders_of(&members, "8").is_empty());
    }

    /// A member Discord sent without a user is not a member anyone can be paid,
    /// so it is not a holder rather than a panic.
    #[test]
    fn a_member_without_a_user_is_skipped() {
        let members = vec![
            json!({ "roles": ["9"] }),
            json!({ "user": { "id": "4" }, "roles": ["9"] }),
        ];

        assert_eq!(holders_of(&members, "9"), ["4".to_owned()]);
    }

    #[test]
    fn the_total_is_the_multiplication_it_cannot_do() {
        assert_eq!(total_for(0, 100), Some(0));
        assert_eq!(total_for(3, 100), Some(300));
        assert_eq!(total_for(3, i64::MAX), None);
    }

    #[tokio::test]
    async fn successful_issuances_return_and_pay_each_holder_once() {
        use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
        use std::sync::{Arc, Mutex};

        async fn issue(
            State(paid): State<Arc<Mutex<Vec<Value>>>>,
            Json(body): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let mut paid = paid.lock().unwrap();
            paid.push(body);
            (
                StatusCode::CREATED,
                Json(json!({
                    "amount": "10", "pool_amount": (100 - paid.len() * 10).to_string(), "unit": "n"
                })),
            )
        }

        let paid = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/api/v2/currencies/issue", post(issue))
            .with_state(paid.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut token = Token {
            access_token: "test-token".into(),
            token_type: "Bearer".into(),
            refresh_token: Some("test-refresh".into()),
        };

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            for (index, receiver) in ["1", "2", "3"].into_iter().enumerate() {
                let issued = issue_to(&client, &service, &mut token, receiver, "10")
                    .await
                    .unwrap();
                assert_eq!(issued.amount, "10");
                assert_eq!(issued.pool_amount, (90 - index * 10).to_string());
                assert_eq!(issued.unit, "n");
            }
        })
        .await
        .unwrap();
        assert_eq!(
            *paid.lock().unwrap(),
            ["1", "2", "3"].map(|receiver| json!({
                "receiver_discord_id": receiver, "amount": "10"
            }))
        );
        server.abort();
    }

    struct Script {
        replies: std::collections::VecDeque<(&'static str, u16, Value)>,
        requests: Vec<(String, String, String)>,
    }

    struct ScriptServer {
        url: String,
        script: std::sync::Arc<std::sync::Mutex<Script>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for ScriptServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn scripted(replies: Vec<(&'static str, u16, Value)>) -> ScriptServer {
        use axum::{Json, Router, extract::Request, http::StatusCode};
        use std::sync::{Arc, Mutex};

        let script = Arc::new(Mutex::new(Script {
            replies: replies.into(),
            requests: Vec::new(),
        }));
        let handler = script.clone();
        let app = Router::new().fallback(move |request: Request| {
            let script = handler.clone();
            async move {
                let path = request.uri().path().to_owned();
                let auth = request
                    .headers()
                    .get("authorization")
                    .map(|value| value.to_str().unwrap().to_owned())
                    .unwrap_or_default();
                let body = axum::body::to_bytes(request.into_body(), 4096)
                    .await
                    .unwrap();
                let mut script = script.lock().unwrap();
                script.requests.push((
                    path.clone(),
                    auth,
                    String::from_utf8(body.to_vec()).unwrap(),
                ));
                let (expected, status, body) =
                    script.replies.pop_front().expect("unexpected request");
                assert_eq!(path, expected);
                (StatusCode::from_u16(status).unwrap(), Json(body))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        ScriptServer { url, script, task }
    }

    fn renewable_token() -> Token {
        Token {
            access_token: "access-0".into(),
            token_type: "Bearer".into(),
            refresh_token: Some("refresh-0".into()),
        }
    }

    fn renewed_token(generation: usize) -> Value {
        json!({
            "access_token": format!("access-{generation}"), "token_type": "Bearer",
            "refresh_token": format!("refresh-{generation}"), "expires_in": 3600
        })
    }

    const ISSUE: &str = "/api/v2/currencies/issue";
    const TOKEN: &str = "/oauth2/token";

    #[tokio::test]
    async fn token_expiry_resumes_the_unpaid_member_and_uses_rotated_refresh_tokens() {
        let issued = json!({"amount":"10", "pool_amount":"70", "unit":"n"});
        let expired = json!({"error":"invalid_token"});
        let server = scripted(vec![
            (ISSUE, 201, issued.clone()),
            (ISSUE, 401, expired.clone()),
            (TOKEN, 200, renewed_token(1)),
            (ISSUE, 201, issued.clone()),
            (ISSUE, 401, expired),
            (TOKEN, 200, renewed_token(2)),
            (ISSUE, 201, issued),
        ])
        .await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut token = renewable_token();
        for receiver in ["1", "2", "3"] {
            let result = issue_to(&client, &server.url, &mut token, receiver, "10")
                .await
                .unwrap();
            assert_eq!(result.amount, "10");
        }
        let script = server.script.lock().unwrap();
        assert!(script.replies.is_empty());
        let issues: Vec<_> = script
            .requests
            .iter()
            .filter(|(path, _, _)| path == ISSUE)
            .collect();
        let receivers: Vec<Value> = issues
            .iter()
            .map(|(_, _, body)| {
                serde_json::from_str::<Value>(body).unwrap()["receiver_discord_id"].clone()
            })
            .collect();
        assert_eq!(receivers, ["1", "2", "2", "3", "3"].map(Value::from));
        assert_eq!(
            issues
                .iter()
                .map(|(_, auth, _)| auth.as_str())
                .collect::<Vec<_>>(),
            [
                "Bearer access-0",
                "Bearer access-0",
                "Bearer access-1",
                "Bearer access-1",
                "Bearer access-2"
            ]
        );
        let refreshes: Vec<_> = script
            .requests
            .iter()
            .filter(|(path, _, _)| path == TOKEN)
            .map(|(_, _, body)| body.as_str())
            .collect();
        assert_eq!(
            refreshes,
            [
                "grant_type=refresh_token&refresh_token=refresh-0",
                "grant_type=refresh_token&refresh_token=refresh-1"
            ]
        );
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-2"));
    }

    #[tokio::test]
    async fn a_failed_refresh_or_unusable_replacement_stops_without_looping() {
        for refresh_fails in [true, false] {
            let mut replies = vec![(ISSUE, 401, json!({"error":"invalid_token"}))];
            if refresh_fails {
                replies.push((TOKEN, 400, json!({"error":"invalid_grant"})));
            } else {
                replies.push((TOKEN, 200, renewed_token(1)));
                replies.push((ISSUE, 401, json!({"error":"invalid_token"})));
            }
            let server = scripted(replies).await;
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let result = issue_to(&client, &server.url, &mut renewable_token(), "2", "10").await;
            assert!(result.is_err());
            assert!(server.script.lock().unwrap().replies.is_empty());
        }
    }
}
