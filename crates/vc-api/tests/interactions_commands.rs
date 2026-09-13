//! The `help` and `invite` commands, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/help_test.exs` and
//! `invite_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

use support::{execute_from_guild, fake, interaction, state, state_with_limiter};
use vc_api::rate_limit::RateLimiter;

const SITE_URL: &str = "https://vcrypto.sumidora.com";
const BOT_INVITE_URL: &str = "https://discord.com/api/oauth2/authorize?client_id=791984306632654869&permissions=0&scope=applications.commands%20bot";
const SUPPORT_GUILD_INVITE_URL: &str = "https://discord.com/invite/Hgp5DpG";

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn help(pool: PgPool) {
    let response = interaction(
        router(pool),
        execute_from_guild(json!({ "name": "help" }), 12),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let description = format!(
        "VirtualCryptoはDiscord上でサーバーに独自の通貨を作成できるBotです。\n\
         [コマンドの使い方の詳細]({SITE_URL}/document/commands)\n\
         [公式サイト]({SITE_URL})\n\
         [Botの招待]({BOT_INVITE_URL})\n\
         [サポートサーバーの招待]({SUPPORT_GUILD_INVITE_URL})"
    );

    assert_eq!(
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "color": 0x0062_21ED,
                    "title": "VirtualCrypto",
                    "thumbnail": { "url": "https://vcrypto.sumidora.com/static/images/logo.jpg" },
                    "description": description,
                }],
            },
        })
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn invite(pool: PgPool) {
    let response = interaction(
        router(pool),
        execute_from_guild(json!({ "name": "invite" }), 12),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let description = format!(
        "[Botの招待]({BOT_INVITE_URL})\n[サポートサーバーの招待]({SUPPORT_GUILD_INVITE_URL})"
    );

    assert_eq!(
        response.body,
        json!({
            "type": 4,
            "data": {
                "flags": 64,
                "embeds": [{
                    "color": 0x0062_21ED,
                    "title": "VirtualCrypto",
                    "thumbnail": { "url": "https://vcrypto.sumidora.com/static/images/logo.jpg" },
                    "description": description,
                }],
            },
        })
    );
}

/// `verified/2` needs `data.name`; without it the interaction falls through to
/// the same `Type Not Found` an unhandled type gets.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_command_without_a_name_is_400(pool: PgPool) {
    let response = interaction(router(pool), execute_from_guild(json!({}), 12)).await;

    assert_eq!(response.status, 400);
    assert_eq!(response.body, Value::String("Type Not Found".into()));
}

/// A name no `Command.handle/4` clause matches. Elixir has no clause either, so
/// it raises and answers 500; this keeps the shape of the response a client can
/// act on, which is the same `Type Not Found` a missing name gets.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_command_is_400(pool: PgPool) {
    let response = interaction(
        router(pool),
        execute_from_guild(json!({ "name": "nonexistent" }), 12),
    )
    .await;

    assert_eq!(response.status, 400);
    assert_eq!(response.body, Value::String("Type Not Found".into()));
}

/// The endpoint holds each Discord user to their own allowance, and the limit is
/// held against them rather than against the address they came from.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_user_is_held_to_their_allowance(pool: PgPool) {
    let api = fake();
    let limiter = Arc::new(RateLimiter::new(1, Duration::from_secs(60)));

    let app = || {
        vc_api::router(state_with_limiter(
            pool.clone(),
            api.clone(),
            limiter.clone(),
        ))
    };

    let first = interaction(app(), execute_from_guild(json!({ "name": "help" }), 12)).await;
    assert_eq!(first.status, 200, "body: {}", first.body);

    let second = interaction(app(), execute_from_guild(json!({ "name": "help" }), 12)).await;
    assert_eq!(second.status, 429, "body: {}", second.body);

    // Another user brings their own allowance.
    let other = interaction(app(), execute_from_guild(json!({ "name": "help" }), 13)).await;
    assert_eq!(other.status, 200, "body: {}", other.body);
}

/// `/application show <client_id>`: the caller's own application, with the secret on the
/// screen rather than behind a button — every response here is ephemeral, so a reveal
/// would show it to the person already reading.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_show_renders_the_callers_own(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;

    let response = interaction(
        router(pool),
        application_payload("show", &client_id, DISCORD),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], 4);
    assert_eq!(
        response.body["data"]["flags"], 32832,
        "ephemeral and components-only"
    );

    // The whole data object as text: the screens are a tree and asserting against it
    // wholesale is how this stays readable when the tree changes.
    let rendered = response.body["data"].to_string();

    assert!(rendered.contains(&client_id), "{rendered}");
    assert!(rendered.contains("テスト"), "{rendered}");
}

/// Somebody else's uuid and a uuid that is not there are the same answer, which is the
/// point of checking ownership before the id rather than after.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_show_does_not_confirm_somebody_elses(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;
    const STRANGER: i64 = 100_000_000_000_000_002;

    support::insert_user(&pool, USER, DISCORD).await;
    let theirs = support::insert_application(&pool, STRANGER, "theirs").await;
    let theirs_id = support::client_id_of(&pool, theirs).await;

    let response = interaction(
        router(pool),
        application_payload("show", &theirs_id, DISCORD),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(!rendered.contains("theirs"), "{rendered}");
    assert!(
        rendered.contains("そのアプリケーションはありません"),
        "{rendered}"
    );
}

/// A `/application <subcommand> <client_id>` interaction, which is what a person running
/// one sends.
fn application_payload(subcommand: &str, client_id: &str, user: i64) -> Value {
    execute_from_guild(
        json!({
            "name": "application",
            "options": [{
                "name": subcommand,
                "type": 1,
                "options": [{ "name": "client_id", "type": 3, "value": client_id }],
            }],
        }),
        user,
    )
}

/// Connecting is a guild's to run: in a DM there is no guild to connect to, and asking for
/// one to be pasted is the thing this whole feature exists to avoid.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_connect_in_a_dm_says_where_to_run_it(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;

    // The same interaction with the guild taken away, which is what a DM is.
    let mut payload = application_payload("connect", &client_id, DISCORD);
    payload["data"]["options"][0]["options"]
        .as_array_mut()
        .expect("the subcommand's options")
        .push(json!({ "name": "bot", "type": 6, "value": "100000000000000002" }));
    payload
        .as_object_mut()
        .expect("an object")
        .remove("guild_id");

    let response = interaction(router(pool), payload).await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(rendered.contains("サーバーの中で行います"), "{rendered}");
    assert!(!rendered.contains("100000000000000002"), "{rendered}");
}

/// `/application edit <client_id>`: the form arrives filled in, because a field the `PATCH`
/// is not sent comes back cleared. This is the property the modal exists for, and until now
/// nothing asserted it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_edit_prefills_what_the_application_has(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;

    let response = interaction(
        router(pool),
        application_payload("edit", &client_id, DISCORD),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], 9, "a modal, not a message");

    let inputs = |name: &str| -> Value {
        response.body["data"]["components"]
            .as_array()
            .expect("the labels")
            .iter()
            .find(|label| label["component"]["custom_id"] == name)
            .map(|label| label["component"].clone())
            .unwrap_or(Value::Null)
    };

    assert_eq!(inputs("client_name")["value"], "テスト");

    // A field the application does not have is sent without a `value` at all, which is what
    // an empty box is. Sending an empty string would be a field the `PATCH` clears.
    assert!(
        inputs("webhook_url").get("value").is_none(),
        "{:?}",
        inputs("webhook_url")
    );
    assert_eq!(inputs("redirect_uris")["value"], "");
}

/// `/application list`: the caller's applications as a menu, keyed by the uuid the service
/// answers with rather than by the name somebody typed.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_list_offers_the_callers_own(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;

    let response = interaction(
        router(pool.clone()),
        execute_from_guild(
            json!({ "name": "application", "options": [{ "name": "list", "type": 1 }] }),
            DISCORD,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], 4);
    assert_eq!(response.body["data"]["flags"], 32832);

    let rendered = response.body["data"].to_string();

    assert!(rendered.contains(&client_id), "{rendered}");
    assert!(rendered.contains("テスト"), "{rendered}");

    // And with nothing to show it says so and offers the form, rather than an empty menu
    // with nothing to pick: an empty state teaches the space.
    let empty = interaction(
        router(pool),
        execute_from_guild(
            json!({ "name": "application", "options": [{ "name": "list", "type": 1 }] }),
            999,
        ),
    )
    .await;

    let rendered = empty.body["data"].to_string();

    assert!(rendered.contains("まだ"), "{rendered}");
    assert!(
        rendered.contains(&support::custom_id_for_register()),
        "{rendered}"
    );
}

/// A form as Discord answers it: the modal's own `custom_id`, and the inputs one `<label>`
/// deep with the values on them.
fn submitted_form(custom_id: &str, fields: Value, user: i64) -> Value {
    json!({
        "type": 5,
        "data": { "custom_id": custom_id, "components": fields },
        "user": { "id": user.to_string() },
    })
}

fn register_form() -> String {
    vc_api::custom_id::ui::developer::custom_id(vc_api::custom_id::ui::developer::Screen::Register)
}

fn field(label: &str, custom_id: &str, value: &str) -> Value {
    json!({
        "type": 18,
        "label": label,
        "component": { "type": 4, "custom_id": custom_id, "value": value },
    })
}

/// A registration form, submitted — an application created from a DM, and the one moment
/// its secret is on a screen.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_submitted_registration_creates_the_application(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;

    let response = interaction(
        router(pool.clone()),
        submitted_form(
            &register_form(),
            json!([
                field("クライアント名", "client_name", "テスト"),
                field(
                    "リダイレクト URI",
                    "redirect_uris",
                    "https://example.test/callback"
                ),
            ]),
            DISCORD,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], 4);
    assert_eq!(response.body["data"]["flags"], 32832, "ephemeral");

    let owned = vc_core::application::owned_by(&pool, USER)
        .await
        .expect("the caller's applications");

    assert_eq!(owned.len(), 1, "{owned:?}");

    let found = vc_api::routes::oauth2_clients::details(&pool, owned[0])
        .await
        .expect("the application could be read")
        .expect("it exists");

    let rendered = response.body["data"].to_string();

    assert!(rendered.contains(&found.client_id), "{rendered}");
    assert!(rendered.contains("テスト"), "{rendered}");

    // The secret is written on the screen because this is the only time it is ever the
    // answer to a question somebody asked.
    let secret = found.client_secret.as_deref().expect("a secret");

    assert!(rendered.contains(secret), "the secret is shown: {rendered}");
}

/// A form that cannot become an application says why, in the service's words — and it is
/// not a message where an application is.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_refused_registration_says_what_is_wrong(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;

    let response = interaction(
        router(pool.clone()),
        submitted_form(
            &register_form(),
            json!([
                field("クライアント名", "client_name", "テスト"),
                // A redirect URI that is not one, which `validated` refuses by name.
                field("リダイレクト URI", "redirect_uris", "ftp://example.test"),
            ]),
            DISCORD,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(
        rendered.contains("redirect_uri_scheme_must_be_http_or_https"),
        "the service's own sentence, not a friendlier one: {rendered}"
    );

    assert!(
        vc_core::application::owned_by(&pool, USER)
            .await
            .expect("the caller's applications")
            .is_empty(),
        "nothing was created"
    );
}

fn edit_form(client_id: &str) -> String {
    vc_api::custom_id::ui::developer::custom_id_for(
        vc_api::custom_id::ui::developer::Screen::Edit,
        client_id,
    )
}

/// An edit submitted from a form: what the form said is what the application has, and the
/// answer is the application as it now is.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_submitted_edit_changes_the_application(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "まえ").await;
    let client_id = support::client_id_of(&pool, application).await;

    let response = interaction(
        router(pool.clone()),
        submitted_form(
            &edit_form(&client_id),
            json!([
                field("クライアント名", "client_name", "あと"),
                field(
                    "リダイレクト URI",
                    "redirect_uris",
                    "https://example.test/after"
                ),
                // Left empty, which is what an untouched optional box arrives as.
                field("webhook URL", "webhook_url", ""),
            ]),
            DISCORD,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], 4);

    let now = vc_api::routes::oauth2_clients::details(&pool, application)
        .await
        .expect("the application could be read")
        .expect("it exists");

    assert_eq!(now.client_name.as_deref(), Some("あと"));
    assert_eq!(
        now.redirect_uris,
        vec!["https://example.test/after".to_owned()]
    );
    assert!(
        now.webhook_url.is_none(),
        "an empty box is cleared, not an empty URL: {:?}",
        now.webhook_url
    );

    // The answer is the application, so the person sees what the edit left behind.
    let rendered = response.body["data"].to_string();

    assert!(rendered.contains("あと"), "{rendered}");
    assert!(rendered.contains(&client_id), "{rendered}");
}
