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
    insert_currency, insert_user, mint, mint_app, rendered_interaction as interaction, state,
};
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

fn router(discord: std::sync::Arc<support::FakeDiscord>, pool: PgPool) -> Router {
    vc_api::router(state(pool, discord))
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

/// Every component the answer carries, counted the way Discord counts them: a container and each
/// of its children, and an accessory where a child has one.
fn components(response: &Response) -> usize {
    fn count(value: &Value) -> usize {
        value.as_array().map_or(0, |items| {
            items
                .iter()
                .map(|item| {
                    1 + count(&item["components"]) + usize::from(item.get("accessory").is_some())
                })
                .sum()
        })
    }

    count(&response.body["data"]["components"])
}

/// The names this account has live tokens under, which is everything a row holds.
async fn names_of(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar!(
        r#"SELECT name AS "name!"
           FROM user_access_tokens
           WHERE user_id = $1 AND name IS NOT NULL
           ORDER BY name"#,
        i64::from(USER)
    )
    .fetch_all(pool)
    .await
    .expect("the token rows")
}

/// The headline: what the command makes is a session token with no end but revocation. It reaches
/// `oauth2.register`, the scope the registration surface is gated on, and one that was not issued
/// with it does not.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_token_carries_the_scopes_the_registration_surface_checks(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;

    let created = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("agent")),
    )
    .await;
    let token = token_of(&created);

    let listed = get(
        router(discord.clone(), pool.clone()),
        LIST_URI,
        Some(&token),
    )
    .await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);

    // The same caller with a token that carries nothing: the endpoint is the scope's, not the
    // account's, so this is what proves the line above was the PAT's scope talking.
    let bare = mint(&pool, USER, &[]).await;
    let refused = get(router(discord.clone(), pool), LIST_URI, Some(&bare)).await;
    assert_ne!(refused.status, 200, "{:?}", refused.body);
}

/// Nothing ends a token but revocation, and both halves say so: the message, and the two places a
/// day could have been recorded — the `exp` claim and the row's `expires`.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_is_named_and_lives_until_it_is_revoked(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;

    let created = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("claude code")),
    )
    .await;

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
            .any(|line| line == "期限はありません。`/pat revoke` で失効させるまで使えます。"),
        "{:?}",
        lines(&created)
    );

    let claims = vc_auth::jwt::verify(&token_of(&created), support::JWT_SECRET.as_bytes())
        .expect("verify token");

    assert_eq!(claims.exp, None, "no claim to expire against");
    assert_eq!(claims.kind, "user");

    let recorded = sqlx::query_scalar!(
        r#"SELECT expires FROM user_access_tokens WHERE user_id = $1 AND name = $2"#,
        i64::from(USER),
        "claude code"
    )
    .fetch_one(&pool)
    .await
    .expect("the token row");

    assert_eq!(recorded, None, "and no row for the purge job to find");
    assert_eq!(names_of(&pool).await, ["claude code"]);
}

/// Revocation is the row: the signature stays valid and the `jti` stops resolving, so the token
/// that answered 200 answers 401 the moment the name is revoked.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_a_name_kills_the_token(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;

    let created = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("agent")),
    )
    .await;
    let token = token_of(&created);

    assert_eq!(
        get(
            router(discord.clone(), pool.clone()),
            LIST_URI,
            Some(&token)
        )
        .await
        .status,
        200
    );

    let revoked = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("revoke", Some("agent")),
    )
    .await;

    assert_eq!(accent(&revoked), COLOR_OK, "{:?}", revoked.body);
    assert_eq!(
        get(
            router(discord.clone(), pool.clone()),
            LIST_URI,
            Some(&token)
        )
        .await
        .status,
        401,
        "the row is gone, so the token is"
    );
    assert!(names_of(&pool).await.is_empty());
}

/// The contract half of what a token can do, which is the party's and not the
/// application's: `docs/contracts.md` puts `vc.contract` on the *application* — it says an
/// application is one that may ask — and says the party answers because it is named. A PAT is a
/// user token, so it is the party: it approves, and it reads the contract among its own.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_pat_answers_as_a_party_to_a_contract(pool: PgPool) {
    let discord = fake();
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
        router(discord.clone(), pool.clone()),
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

    let made = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("agent")),
    )
    .await;
    let token = token_of(&made);

    let approved = request(
        router(discord.clone(), pool.clone()),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        &token,
        Value::Null,
    )
    .await;

    assert_eq!(approved.status, 200, "{:?}", approved.body);
    assert_eq!(approved.body["parties"][0]["status"], "approved");

    let mine = get(
        router(discord.clone(), pool),
        "/api/v2/users/@me/contracts",
        Some(&token),
    )
    .await;

    assert_eq!(mine.status, 200, "{:?}", mine.body);
    assert_eq!(mine.body.as_array().map(Vec::len), Some(1));
}

/// A name is how a token is revoked, so two of them on one account could not be told apart: the
/// second is refused rather than made, and the first keeps working.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_name_can_only_be_used_once(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;

    let first = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("agent")),
    )
    .await;
    let again = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("agent")),
    )
    .await;

    assert_eq!(accent(&again), COLOR_ERROR, "{:?}", again.body);
    assert_eq!(names_of(&pool).await.len(), 1, "the first one is still it");
    assert_eq!(
        get(
            router(discord.clone(), pool),
            LIST_URI,
            Some(&token_of(&first))
        )
        .await
        .status,
        200
    );
}

/// What Discord's option already refuses, checked here as well, because the option's bound is a
/// statement about the command and this is the code that has to make it true.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_name_longer_than_the_option_allows_is_refused(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;

    let long = "a".repeat(33);
    let refused = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some(&long)),
    )
    .await;

    assert_eq!(accent(&refused), COLOR_ERROR, "{:?}", refused.body);
    assert!(
        lines(&refused)
            .iter()
            .any(|line| line == "名前は1〜32文字です。"),
        "{:?}",
        lines(&refused)
    );
    assert!(names_of(&pool).await.is_empty(), "and nothing was made");
}

/// A list is for recognising a credential, not for reading it back: it answers with the name and
/// nothing else, and the token itself is one place only — the message that made it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_names_tokens_and_never_shows_one(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;

    let empty = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("list", None),
    )
    .await;
    assert_eq!(accent(&empty), COLOR_BRAND, "{:?}", empty.body);
    assert!(
        lines(&empty)[0].contains("/pat create"),
        "an empty state teaches the space: {:?}",
        lines(&empty)
    );

    let created = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("agent")),
    )
    .await;
    let token = token_of(&created);

    let second = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("claude code")),
    )
    .await;
    assert_eq!(accent(&second), COLOR_OK, "{:?}", second.body);

    let listed = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        pat("list", None),
    )
    .await;

    assert_eq!(
        lines(&listed),
        ["**agent**", "**claude code**"],
        "the names, in name order, and nothing else"
    );
    assert!(
        !lines(&listed).join("\n").contains(&token),
        "the token is not in a list"
    );
}

/// The list is one message and nothing pages it, so the count is bounded: the twenty-sixth token
/// is refused, the refusal says where to look and what to do instead, and the list the person is
/// sent to still holds every name.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_account_holds_at_most_twenty_five_tokens(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;

    for index in 0..25 {
        let made = interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            pat("create", Some(&format!("t{index:02}"))),
        )
        .await;

        assert_eq!(accent(&made), COLOR_OK, "{:?}", made.body);
    }

    let refused = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("one too many")),
    )
    .await;

    assert_eq!(accent(&refused), COLOR_ERROR, "{:?}", refused.body);

    let sentence = &lines(&refused)[0];

    assert!(sentence.contains("25個まで"), "{sentence}");
    assert!(
        sentence.contains("/pat list") && sentence.contains("/pat revoke"),
        "the way to find what to give up, and what to do about it: {sentence}"
    );
    assert_eq!(names_of(&pool).await.len(), 25, "and nothing more was made");

    let listed = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        pat("list", None),
    )
    .await;

    assert_eq!(
        lines(&listed).len(),
        25,
        "all of them, on the one screen that is why there is a limit"
    );
    assert!(
        components(&listed) <= 40,
        "the whole list has to fit one message, and Discord allows forty components: {}",
        components(&listed)
    );
}

/// A token belongs to an account, so a caller who has none is told which step makes one rather
/// than being handed a credential that would resolve against nothing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_caller_without_an_account_is_told_so(pool: PgPool) {
    let discord = fake();
    let created = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("agent")),
    )
    .await;

    assert_eq!(accent(&created), COLOR_ERROR, "{:?}", created.body);
    assert!(
        lines(&created)[0].contains("アカウントがまだありません"),
        "{:?}",
        lines(&created)
    );
    assert!(names_of(&pool).await.is_empty());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn concurrent_pat_creation_respects_limit(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;
    for index in 0..24 {
        let response = interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            pat("create", Some(&format!("token{index}"))),
        )
        .await;
        assert_eq!(
            response.body["data"]["components"][0]["accent_color"],
            COLOR_OK
        );
    }
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE user_access_tokens IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let a = tokio::spawn(interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("parallel-a")),
    ));
    let b = tokio::spawn(interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("parallel-b")),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND (query LIKE 'INSERT INTO user_access_tokens%' OR query LIKE 'SELECT id FROM users%FOR UPDATE')")
                .fetch_one(&pool).await.unwrap();
            if waiting == 2 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("both requests reached the issuance locks");
    blocker.commit().await.unwrap();
    let a = a.await.unwrap();
    let b = b.await.unwrap();
    assert_eq!(a.status, 202);
    assert_eq!(b.status, 202);
    let mut colors = [
        a.body["data"]["components"][0]["accent_color"]
            .as_i64()
            .unwrap(),
        b.body["data"]["components"][0]["accent_color"]
            .as_i64()
            .unwrap(),
    ];
    colors.sort();
    let mut expected = [COLOR_OK, COLOR_ERROR];
    expected.sort();
    assert_eq!(colors, expected);
    assert_eq!(
        names_of(&pool).await.len(),
        25,
        "24 tokens followed by two concurrent creations must stay within the cap"
    );
}

async fn pat_registration(
    pool: &PgPool,
    discord: std::sync::Arc<support::FakeDiscord>,
) -> Response {
    let made = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("agent")),
    )
    .await;
    assert_eq!(accent(&made), COLOR_OK);
    request(
        vc_api::router(state(pool.clone(), discord)),
        "POST",
        "/oauth2/clients",
        &token_of(&made),
        json!({"client_name":"agent app", "redirect_uris":["https://example.com/callback"]}),
    )
    .await
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pat_registers_and_reads_profile_without_browser_login(pool: PgPool) {
    insert_user(&pool, USER, DISCORD_ID).await;
    let discord = std::sync::Arc::new(support::FakeDiscord::with_user_profile(
        json!({"username":"tester", "bot":false,"extra_field_that_must_be_filtered":"ignored"})
            .as_object()
            .unwrap()
            .clone(),
    ));
    let registered = pat_registration(&pool, discord.clone()).await;
    assert_eq!(registered.status, 201);
    let owner: i64 =
        sqlx::query_scalar("SELECT owner_discord_id FROM applications WHERE client_id = $1::uuid")
            .bind(registered.body["client_id"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(owner, DISCORD_ID);
    let made = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("profile")),
    )
    .await;
    let me = get(
        vc_api::router(state(pool, discord)),
        "/api/v2/users/@me",
        Some(&token_of(&made)),
    )
    .await;
    assert_eq!(me.status, 200);
    assert_eq!(me.body["id"], USER.to_string());
    assert_eq!(me.body["discord"]["id"], DISCORD_ID.to_string());
    assert_eq!(me.body["discord"]["username"], "tester");
    assert!(
        me.body["discord"]
            .get("extra_field_that_must_be_filtered")
            .is_none()
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pat_works_with_an_unusable_browser_authorization(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;
    support::insert_discord_auth(&pool, DISCORD_ID, "old-token").await;
    support::set_discord_updated_at(
        &pool,
        DISCORD_ID,
        support::utc_now() - time::Duration::days(8),
    )
    .await;
    sqlx::query("UPDATE discord_users SET refresh_token = NULL WHERE discord_user_id = $1")
        .bind(DISCORD_ID)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(pat_registration(&pool, fake()).await.status, 201);
    let made = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("profile")),
    )
    .await;
    assert_eq!(
        get(
            router(discord.clone(), pool),
            "/api/v2/users/@me",
            Some(&token_of(&made))
        )
        .await
        .status,
        200
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pat_registration_still_refuses_bots(pool: PgPool) {
    insert_user(&pool, USER, DISCORD_ID).await;
    let discord = std::sync::Arc::new(support::FakeDiscord::with_user_profile(
        json!({"bot":true}).as_object().unwrap().clone(),
    ));
    let refused = pat_registration(&pool, discord).await;
    assert_eq!(refused.status, 400);
    assert_eq!(refused.body["error"], "user_verification_failed");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM applications")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pat_registration_requires_a_successful_profile_lookup(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;
    let made = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("create", Some("agent")),
    )
    .await;
    let token = token_of(&made);
    for status in [404, 503] {
        let discord = std::sync::Arc::new(support::FakeDiscord::with_user_status(status));
        let refused = request(
            vc_api::router(state(pool.clone(), discord)),
            "POST",
            "/oauth2/clients",
            &token,
            json!({"redirect_uris":["https://example.com/callback"]}),
        )
        .await;
        assert_eq!(refused.status, 500);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM applications")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
