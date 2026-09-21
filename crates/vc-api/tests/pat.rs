//! Personal access tokens: `/pat`, and what the token it makes is.
//!
//! An addition rather than a port — the Elixir has no personal access token, no API key, and no
//! column that could hold one — so nothing here has an Elixir case behind it: `docs/pat.md` is the
//! design and this file is what holds it to that.
//!
//! What the token *is* matters more than what the command says: a `kind: user` token carrying the
//! browser session's scopes, which is why the two tests that matter are the ones asking the API —
//! `oauth2.register` is the scope the registration surface checks, and revocation is the row.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    Response, account_of, execute_from_dm, fake, get, insert_application, insert_asset,
    insert_currency, insert_user, interaction, mint, mint_app, state,
};
use time::{OffsetDateTime, PrimitiveDateTime};
use tower::ServiceExt;

const DISCORD_ID: i64 = 100_000_000_000_000_001;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const GUILD: i64 = 900_000_000_000_000_001;
const USER: i32 = 1;
const COLOR_BRAND: i64 = 0x0062_21ED;
const COLOR_OK: i64 = 0x0038_EA42;
const COLOR_ERROR: i64 = 0x00EA_3875;
const LIST_URI: &str = "/oauth2/clients";

/// A request with a body, which the shared support has no shape for: the contract and claim
/// suites each keep their own, and this is that one, for the two calls a party makes.
async fn request(app: Router, method: &str, uri: &str, token: &str, body: Value) -> Response {
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .header("accept", "application/json")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(body.to_string()))
                .expect("a request"),
        )
        .await
        .expect("a response");

    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body");

    Response {
        status,
        headers,
        body: if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("a json body")
        },
    }
}

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// A `/pat` interaction, typed by the account's own user in a direct message.
fn pat(subcommand: &str, name: Option<&str>) -> Value {
    let options: Vec<Value> = match name {
        Some(name) => vec![json!({ "name": "name", "value": name })],
        None => vec![],
    };

    execute_from_dm(
        json!({
            "name": "pat",
            "options": [{ "name": subcommand, "type": 1, "options": options }]
        }),
        DISCORD_ID,
    )
}

/// The screen's text lines, in order, which is what a person reads.
fn lines(response: &Response) -> Vec<String> {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("a container")
        .iter()
        .filter_map(|component| component["content"].as_str().map(str::to_string))
        .collect()
}

/// The accent the screen carries, which is how the command says whether it worked.
fn accent(response: &Response) -> i64 {
    response.body["data"]["components"][0]["accent_color"]
        .as_i64()
        .expect("an accent")
}

/// The token `create` answered with: the one fenced block in the screen.
fn token_of(response: &Response) -> String {
    let line = lines(response)
        .into_iter()
        .find(|line| line.starts_with("```\n") && line.ends_with("\n```"))
        .expect("a fenced token");

    line.trim_matches('`').trim().to_string()
}

async fn options_of(pool: &PgPool) -> Vec<(String, PrimitiveDateTime)> {
    let rows = sqlx::query!(
        r#"SELECT name AS "name!", expires AS "expires!"
           FROM user_access_tokens
           WHERE user_id = $1 AND name IS NOT NULL
           ORDER BY name"#,
        i64::from(USER)
    )
    .fetch_all(pool)
    .await
    .expect("the token rows");

    rows.into_iter()
        .map(|row| (row.name, row.expires))
        .collect()
}

/// The headline: what the command makes is a session token with a longer life. It reaches
/// `oauth2.register`, the scope the registration surface is gated on, and one that was not issued
/// with it does not.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_token_carries_the_scopes_the_registration_surface_checks(pool: PgPool) {
    insert_user(&pool, USER, DISCORD_ID).await;

    let created = interaction(router(pool.clone()), pat("create", Some("agent"))).await;
    let token = token_of(&created);

    let listed = get(router(pool.clone()), LIST_URI, Some(&token)).await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);

    // The same caller with a token that carries nothing: the endpoint is the scope's, not the
    // account's, so this is what proves the line above was the PAT's scope talking.
    let bare = mint(&pool, USER, &[]).await;
    let refused = get(router(pool), LIST_URI, Some(&bare)).await;
    assert_ne!(refused.status, 200, "{:?}", refused.body);
}

/// The year is the point of it, and the row says so: the purge job reads this column, so a token
/// that outlives its row would be one that is never cleaned up.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_is_named_and_stored_for_a_year(pool: PgPool) {
    insert_user(&pool, USER, DISCORD_ID).await;

    let created = interaction(router(pool.clone()), pat("create", Some("claude code"))).await;

    assert_eq!(accent(&created), COLOR_OK, "{:?}", created.body);
    assert!(token_of(&created).matches('.').count() >= 2, "a JWT");
    assert!(
        lines(&created)
            .iter()
            .any(|line| line == "スコープ: oauth2.register, vc.pay, vc.claim"),
        "{:?}",
        lines(&created)
    );
    assert!(
        lines(&created)
            .iter()
            .any(|line| line.starts_with("有効期限: <t:")),
        "{:?}",
        lines(&created)
    );

    let stored = options_of(&pool).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].0, "claude code");

    // And the day it stops working is about a year out, which is what the column says rather than
    // what the message says.
    let days = (stored[0].1.assume_utc() - OffsetDateTime::now_utc()).whole_days();

    assert!((364..=365).contains(&days), "{days} days");
}

/// Revocation is the row: the signature stays valid and the `jti` stops resolving, so the token
/// that answered 200 answers 401 the moment the name is revoked.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_a_name_kills_the_token(pool: PgPool) {
    insert_user(&pool, USER, DISCORD_ID).await;

    let created = interaction(router(pool.clone()), pat("create", Some("agent"))).await;
    let token = token_of(&created);

    assert_eq!(
        get(router(pool.clone()), LIST_URI, Some(&token))
            .await
            .status,
        200
    );

    let revoked = interaction(router(pool.clone()), pat("revoke", Some("agent"))).await;

    assert_eq!(accent(&revoked), COLOR_OK, "{:?}", revoked.body);
    assert_eq!(
        get(router(pool.clone()), LIST_URI, Some(&token))
            .await
            .status,
        401,
        "the row is gone, so the token is"
    );
    assert!(options_of(&pool).await.is_empty());
}

/// The contract half of what a token can do, which is the party's and not the
/// application's: `docs/contracts.md` puts `vc.contract` on the *application* — it says an
/// application is one that may ask — and says the party answers because it is named. A PAT is a
/// user token, so it is the party: it approves, and it reads the contract among its own.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_pat_answers_as_a_party_to_a_contract(pool: PgPool) {
    insert_user(&pool, USER, DISCORD_ID).await;
    insert_currency(&pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(&pool, USER, 1, 1_000).await;

    let application = insert_application(&pool, OWNER_DISCORD_ID, "an application").await;
    let writer = mint_app(
        &pool,
        account_of(&pool, application).await,
        &["vc.contract"],
    )
    .await;

    let created = request(
        router(pool.clone()),
        "POST",
        "/api/v2/contracts",
        &writer,
        json!({
            "unit": "nyan",
            "parties": [{ "discord_id": DISCORD_ID.to_string(), "amount": "100" }]
        }),
    )
    .await;

    assert_eq!(created.status, 201, "{:?}", created.body);
    let id = created.body["id"].as_str().expect("a contract id");

    let made = interaction(router(pool.clone()), pat("create", Some("agent"))).await;
    let token = token_of(&made);

    let approved = request(
        router(pool.clone()),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        &token,
        Value::Null,
    )
    .await;

    assert_eq!(approved.status, 200, "{:?}", approved.body);
    assert_eq!(approved.body["parties"][0]["status"], "approved");

    let mine = get(router(pool), "/api/v2/users/@me/contracts", Some(&token)).await;

    assert_eq!(mine.status, 200, "{:?}", mine.body);
    assert_eq!(mine.body.as_array().map(Vec::len), Some(1));
}

/// A name is how a token is revoked, so two of them on one account could not be told apart: the
/// second is refused rather than made, and the first keeps working.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_name_can_only_be_used_once(pool: PgPool) {
    insert_user(&pool, USER, DISCORD_ID).await;

    let first = interaction(router(pool.clone()), pat("create", Some("agent"))).await;
    let again = interaction(router(pool.clone()), pat("create", Some("agent"))).await;

    assert_eq!(accent(&again), COLOR_ERROR, "{:?}", again.body);
    assert_eq!(
        options_of(&pool).await.len(),
        1,
        "the first one is still it"
    );
    assert_eq!(
        get(router(pool), LIST_URI, Some(&token_of(&first)))
            .await
            .status,
        200
    );
}

/// What Discord's option already refuses, checked here as well, because the option's bound is a
/// statement about the command and this is the code that has to make it true.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_name_longer_than_the_option_allows_is_refused(pool: PgPool) {
    insert_user(&pool, USER, DISCORD_ID).await;

    let long = "a".repeat(33);
    let refused = interaction(router(pool.clone()), pat("create", Some(&long))).await;

    assert_eq!(accent(&refused), COLOR_ERROR, "{:?}", refused.body);
    assert!(
        lines(&refused)
            .iter()
            .any(|line| line == "名前は1〜32文字です。"),
        "{:?}",
        lines(&refused)
    );
    assert!(options_of(&pool).await.is_empty(), "and nothing was made");
}

/// A list is for recognising a credential, not for reading it back: it answers with the name and
/// the day, and the token itself is one place only — the message that made it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_names_tokens_and_never_shows_one(pool: PgPool) {
    insert_user(&pool, USER, DISCORD_ID).await;

    let empty = interaction(router(pool.clone()), pat("list", None)).await;
    assert_eq!(accent(&empty), COLOR_BRAND, "{:?}", empty.body);
    assert!(
        lines(&empty)[0].contains("/pat create"),
        "an empty state teaches the space: {:?}",
        lines(&empty)
    );

    let created = interaction(router(pool.clone()), pat("create", Some("agent"))).await;
    let token = token_of(&created);

    let listed = interaction(router(pool.clone()), pat("list", None)).await;
    let text = lines(&listed).join("\n");

    assert!(text.contains("agent"), "{text}");
    assert!(
        text.contains("有効期限 <t:"),
        "the day it expires, and nothing else: {text}"
    );
    assert!(!text.contains(&token), "the token is not in a list: {text}");
}

/// A token belongs to an account, so a caller who has none is told which step makes one rather
/// than being handed a credential that would resolve against nothing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_caller_without_an_account_is_told_so(pool: PgPool) {
    let created = interaction(router(pool.clone()), pat("create", Some("agent"))).await;

    assert_eq!(accent(&created), COLOR_ERROR, "{:?}", created.body);
    assert!(
        lines(&created)[0].contains("アカウントがまだありません"),
        "{:?}",
        lines(&created)
    );
    assert!(options_of(&pool).await.is_empty());
}
