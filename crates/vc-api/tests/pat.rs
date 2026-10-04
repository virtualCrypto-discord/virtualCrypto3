//! Personal access tokens: `/pat`, and what the token it makes is.
//!
//! An addition rather than a port — the Elixir has no personal access token, no API key, and no
//! column that could hold one — so nothing here has an Elixir case behind it: `docs/pat.md` is the
//! design and this file is what holds it to that.
//!
//! A `kind: user` token with fixed account scopes. The API checks are essential:
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

fn rows(response: &Response) -> Vec<&Value> {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|component| component["type"] == 9)
        .collect()
}

fn revoke_button(response: &Response, name: &str) -> String {
    rows(response)
        .into_iter()
        .find(|row| row["components"][0]["content"] == format!("**{name}**"))
        .expect("the named row")["accessory"]["custom_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn navigation(response: &Response, emoji: &str) -> Value {
    let button = response.body["data"]["components"][0]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["type"] == 1)
        .unwrap()["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|button| button["emoji"]["name"] == emoji)
        .unwrap();
    assert!(button.get("label").is_none());
    button.clone()
}

fn press(custom_id: &str, user: i64) -> Value {
    json!({"type":3,"user":{"id":user.to_string()},
        "data":{"component_type":2,"custom_id":custom_id}})
}

fn assert_private(response: &Response) {
    assert_eq!(response.body["data"]["flags"].as_u64().unwrap() & 64, 64);
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

/// The command makes an account token with no end but revocation. It reaches
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
            .any(|line| line.contains("有効期限はありません")
                && line.contains("/pat list")
                && line.contains("失効")),
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
/// that answered 200 answers 401 the moment its list button is pressed.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_from_the_list_kills_the_token(pool: PgPool) {
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

    let listed = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("list", None),
    )
    .await;
    let revoked = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        press(&revoke_button(&listed, "agent"), DISCORD_ID),
    )
    .await;

    assert_eq!(accent(&revoked), COLOR_OK, "{:?}", revoked.body);
    assert_eq!(revoked.body["type"], 7, "updates the same private list");
    assert_private(&revoked);
    assert!(rows(&revoked).is_empty());
    assert!(
        lines(&revoked)
            .iter()
            .any(|line| line.contains("/pat create"))
    );
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

/// Duplicate names are refused so a user can distinguish the list's revocation targets.
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
    assert_eq!(
        lines(&again)[0],
        "**エラー**\n**agent** という名前のトークンはすでにあります。別の名前にしてください。"
    );
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
            .any(|line| line == "**エラー**\n名前は1〜32文字で指定してください。"),
        "{:?}",
        lines(&refused)
    );
    assert!(names_of(&pool).await.is_empty(), "and nothing was made");
}

/// A list is for recognising a credential, not for reading it back: it answers with the name and
/// revocation buttons, and the token itself is one place only — the message that made it.
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
        rows(&listed)
            .iter()
            .map(|row| row["components"][0]["content"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["**agent**", "**claude code**"],
        "the names, in name order, and nothing else"
    );
    assert!(
        !listed.body.to_string().contains(&token),
        "the token is not in a list"
    );
    assert_private(&created);
    assert_private(&listed);
    for row in rows(&listed) {
        assert_eq!(row["accessory"]["label"], "失効");
        assert_eq!(row["accessory"]["style"], 4);
    }
}

/// The cap and every page of revocation buttons remain usable together.
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
        sentence.contains("/pat list") && sentence.contains("失効させてください"),
        "the way to find what to give up, and what to do about it: {sentence}"
    );
    assert_eq!(names_of(&pool).await.len(), 25, "and nothing more was made");

    let mut listed = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        pat("list", None),
    )
    .await;

    assert_eq!(navigation(&listed, "⏮️")["disabled"], true);
    let mut seen = Vec::new();
    for expected in [10, 10, 5] {
        assert_private(&listed);
        assert_eq!(rows(&listed).len(), expected);
        assert!(components(&listed) <= 40);
        seen.extend(
            rows(&listed)
                .into_iter()
                .map(|row| row["components"][0]["content"].as_str().unwrap().to_owned()),
        );
        let next = navigation(&listed, "⏭️");
        if next["disabled"] == true {
            break;
        }
        listed = interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            press(next["custom_id"].as_str().unwrap(), DISCORD_ID),
        )
        .await;
        assert_eq!(listed.body["type"], 7);
    }
    assert_eq!(
        seen,
        (0..25).map(|n| format!("**t{n:02}**")).collect::<Vec<_>>()
    );
    assert_eq!(navigation(&listed, "⏭️")["disabled"], true);

    // Shrinking away the last page returns to the preceding page automatically.
    for n in 20..25 {
        listed = interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            press(&revoke_button(&listed, &format!("t{n:02}")), DISCORD_ID),
        )
        .await;
        assert_eq!(accent(&listed), COLOR_OK);
        assert!(components(&listed) <= 40);
    }
    assert!(
        lines(&listed)
            .iter()
            .any(|line| line.contains("2 / 2ページ"))
    );
    assert_eq!(rows(&listed).len(), 10);
    let previous = navigation(&listed, "⏮️");
    listed = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        press(previous["custom_id"].as_str().unwrap(), DISCORD_ID),
    )
    .await;
    assert!(
        lines(&listed)
            .iter()
            .any(|line| line.contains("1 / 2ページ"))
    );
    let created = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        pat("create", Some("replacement")),
    )
    .await;
    assert_eq!(accent(&created), COLOR_OK, "a revoked token frees a slot");
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
async fn old_buttons_cannot_revoke_a_replacement_with_the_same_name(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;
    let app = router(discord.clone(), pool.clone());
    interaction(discord.clone(), app.clone(), pat("create", Some("agent"))).await;
    let listed = interaction(discord.clone(), app.clone(), pat("list", None)).await;
    let old_button = revoke_button(&listed, "agent");
    let revoked = interaction(discord.clone(), app.clone(), press(&old_button, DISCORD_ID)).await;
    assert_eq!(accent(&revoked), COLOR_OK);
    let repeated = interaction(discord.clone(), app.clone(), press(&old_button, DISCORD_ID)).await;
    assert_eq!(accent(&repeated), COLOR_ERROR);
    let replacement = interaction(discord.clone(), app.clone(), pat("create", Some("agent"))).await;
    let stale = interaction(discord.clone(), app.clone(), press(&old_button, DISCORD_ID)).await;
    assert_eq!(accent(&stale), COLOR_ERROR);
    assert_eq!(rows(&stale).len(), 1);
    assert_ne!(revoke_button(&stale, "agent"), old_button);
    assert_eq!(
        get(app, LIST_URI, Some(&token_of(&replacement)))
            .await
            .status,
        200
    );
    assert_eq!(names_of(&pool).await, ["agent"]);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn buttons_cannot_revoke_another_accounts_or_an_unnamed_token(pool: PgPool) {
    use vc_api::custom_id::ui::pat as ids;
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;
    insert_user(&pool, 2, OWNER_DISCORD_ID).await;
    let app = router(discord.clone(), pool.clone());
    let made = interaction(discord.clone(), app.clone(), pat("create", Some("agent"))).await;
    let listed = interaction(discord.clone(), app.clone(), pat("list", None)).await;
    let token_id = vc_auth::issue::personal_tokens(&pool, i64::from(USER))
        .await
        .unwrap()[0]
        .token_id;
    // Both a copied button and one forged to name the attacker's own Discord id fail.
    for id in [
        revoke_button(&listed, "agent"),
        ids::revoke(OWNER_DISCORD_ID, 1, token_id),
    ] {
        let response =
            interaction(discord.clone(), app.clone(), press(&id, OWNER_DISCORD_ID)).await;
        assert_eq!(accent(&response), COLOR_ERROR);
        assert_private(&response);
        assert!(!response.body.to_string().contains("**agent**"));
    }
    let other_page = interaction(
        discord.clone(),
        app.clone(),
        press(&ids::page(DISCORD_ID, 1), OWNER_DISCORD_ID),
    )
    .await;
    assert_eq!(accent(&other_page), COLOR_ERROR);
    assert!(!other_page.body.to_string().contains("**agent**"));
    assert_eq!(
        get(app.clone(), LIST_URI, Some(&token_of(&made)))
            .await
            .status,
        200
    );

    let unnamed = mint(&pool, USER, &["oauth2.register"]).await;
    let claims = vc_auth::jwt::verify(&unnamed, support::JWT_SECRET.as_bytes()).unwrap();
    let response = interaction(
        discord.clone(),
        app.clone(),
        press(
            &ids::revoke(DISCORD_ID, 1, claims.jti.parse().unwrap()),
            DISCORD_ID,
        ),
    )
    .await;
    assert_eq!(accent(&response), COLOR_ERROR);
    assert_eq!(get(app, LIST_URI, Some(&unnamed)).await.status, 200);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn unicode_names_at_the_boundary_can_be_revoked_from_the_list(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;
    let app = router(discord.clone(), pool.clone());
    let name = "名".repeat(32);
    let made = interaction(discord.clone(), app.clone(), pat("create", Some(&name))).await;
    assert_eq!(accent(&made), COLOR_OK);
    let listed = interaction(discord.clone(), app.clone(), pat("list", None)).await;
    let id = revoke_button(&listed, &name);
    assert!(id.encode_utf16().count() <= 100);
    let revoked = interaction(discord.clone(), app.clone(), press(&id, DISCORD_ID)).await;
    assert_eq!(accent(&revoked), COLOR_OK);
    for name in ["名".repeat(33), "   ".to_owned()] {
        let refused = interaction(discord.clone(), app.clone(), pat("create", Some(&name))).await;
        assert_eq!(accent(&refused), COLOR_ERROR);
    }
    assert!(names_of(&pool).await.is_empty());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn guild_creation_is_private_and_explains_the_authority_and_one_time_display(pool: PgPool) {
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;
    let app = router(discord.clone(), pool);
    let mut payload = pat("create", Some("**tool**"));
    payload["member"] = json!({"user":{"id":DISCORD_ID.to_string()}});
    payload["guild_id"] = json!(GUILD.to_string());
    payload.as_object_mut().unwrap().remove("user");
    let made = interaction(discord.clone(), app.clone(), payload).await;
    assert_eq!(accent(&made), COLOR_OK);
    assert_private(&made);
    let prose = lines(&made).join("\n");
    for meaning in [
        "一度しか表示されません",
        "有効期限はありません",
        "あなたとして",
        "送金",
        "他人に見せないでください",
        "/pat list",
    ] {
        assert!(prose.contains(meaning), "missing explanation: {meaning}");
    }
    assert!(!prose.contains("ブラウザ"));
    let listed = interaction(discord.clone(), app, pat("list", None)).await;
    assert_private(&listed);
    assert!(!listed.body.to_string().contains(&token_of(&made)));
    assert_eq!(
        rows(&listed)[0]["components"][0]["content"],
        "**\\*\\*tool\\*\\***"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn obsolete_commands_and_invalid_component_payloads_do_not_revoke(pool: PgPool) {
    use vc_api::custom_id::ui::pat as ids;
    let discord = fake();
    insert_user(&pool, USER, DISCORD_ID).await;
    let app = router(discord.clone(), pool.clone());
    interaction(discord.clone(), app.clone(), pat("create", Some("agent"))).await;
    let listed = interaction(discord.clone(), app.clone(), pat("list", None)).await;
    let mut wrong_type = press(&revoke_button(&listed, "agent"), DISCORD_ID);
    wrong_type["data"]["component_type"] = json!(3);
    for payload in [
        pat("revoke", Some("agent")),
        wrong_type,
        press(&ids::page(DISCORD_ID, 0), DISCORD_ID),
    ] {
        let refused = interaction(discord.clone(), app.clone(), payload).await;
        assert_eq!(refused.status, 400);
    }
    assert_eq!(names_of(&pool).await, ["agent"]);
}

#[test]
fn pat_registration_and_help_offer_create_and_list() {
    let registered = vc_api::discord_commands::commands()
        .into_iter()
        .find(|command| command["name"] == "pat")
        .unwrap();
    assert_eq!(
        registered["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["create", "list"]
    );
    let doc = vc_api::docs::commands::all()
        .iter()
        .find(|command| command.name == "pat")
        .unwrap();
    assert_eq!(doc.usage, ["/pat create name:<名前>", "/pat list"]);
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
