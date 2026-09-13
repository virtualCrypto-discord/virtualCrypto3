//! The `help` and `invite` commands, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/help_test.exs` and
//! `invite_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

use support::{execute_from_dm, execute_from_guild, fake, interaction, state, state_with_limiter};
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
        rendered.contains("/application register"),
        "an empty list teaches the command: {rendered}"
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

fn chose(custom_id: &str, value: &str, user: i64) -> Value {
    json!({
        "type": 3,
        "data": { "custom_id": custom_id, "component_type": 3, "values": [value] },
        "user": { "id": user.to_string() },
    })
}

/// The `custom_id` of the menu on a screen, which is a string select on the list and a user
/// select on an application.
fn select_id(screen: &Value) -> String {
    fn walk(value: &Value, found: &mut Option<String>) {
        match value {
            Value::Object(map) => {
                if map
                    .get("type")
                    .and_then(Value::as_i64)
                    .is_some_and(|kind| kind == 3 || kind == 5)
                {
                    *found = map
                        .get("custom_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }

                for nested in map.values() {
                    walk(nested, found);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, found);
                }
            }
            _ => {}
        }
    }

    let mut found = None;
    walk(screen, &mut found);

    found.unwrap_or_else(|| panic!("no menu in {screen}"))
}

/// The whole walk: the list offers a menu, the choice is the application, the application
/// offers the form, and the form is about that application.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_menu_choice_is_the_application_and_its_form_is_about_it(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;

    let listed = interaction(
        router(pool.clone()),
        execute_from_dm(
            json!({ "name": "application", "options": [{ "name": "list", "type": 1 }] }),
            DISCORD,
        ),
    )
    .await;

    assert_eq!(listed.status, 200, "body: {}", listed.body);

    let chosen = interaction(
        router(pool.clone()),
        chose(&select_id(&listed.body["data"]), &client_id, DISCORD),
    )
    .await;

    assert_eq!(chosen.status, 200, "body: {}", chosen.body);
    assert_eq!(
        chosen.body["type"], 7,
        "a change to the message, not another one: {}",
        chosen.body
    );

    assert_eq!(
        select_id(&chosen.body["data"]),
        vc_api::custom_id::ui::developer::custom_id_for(
            vc_api::custom_id::ui::developer::Screen::Connect,
            &client_id,
        ),
        "the screen offers the bot picker for this application"
    );
}

/// Connect, which is a guild's: the id is already in the interaction, so nothing is typed,
/// and the proof is Discord's — the guild's integration for this bot says the client id.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_connect_in_a_guild_binds_the_bot(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;
    const BOT: i64 = 200_000_000_000_000_002;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;

    let discord = Arc::new(support::FakeDiscord::with_integrations(
        json!({ "name": "TestGuild" }),
        &[(BOT, &client_id)],
    ));

    let response = interaction(
        vc_api::router(support::state(pool.clone(), discord)),
        connect_payload(&client_id, BOT, DISCORD),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(rendered.contains("接続しました"), "{rendered}");

    // The bot's id is on the application's own account, which is what an integration with no
    // Discord id was missing.
    let bound = sqlx::query_scalar!(
        "SELECT discord_id FROM users WHERE application_id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the account");

    assert_eq!(bound, Some(BOT));
}

/// A bot whose integration does not name this application, and the answer is the service's
/// own sentence about it — which is the thing a person has to act on, unchanged.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_connect_says_why_a_bot_is_refused(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;
    const BOT: i64 = 200_000_000_000_000_002;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;

    let discord = Arc::new(support::FakeDiscord::with_integrations(
        json!({ "name": "TestGuild" }),
        &[(BOT, "another application's client id")],
    ));

    let response = interaction(
        vc_api::router(support::state(pool.clone(), discord)),
        connect_payload(&client_id, BOT, DISCORD),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(
        rendered.contains(
            "the integration's description does not contain this application's client id"
        ),
        "the service's own sentence, not a friendlier one: {rendered}"
    );

    let bound = sqlx::query_scalar!(
        "SELECT discord_id FROM users WHERE application_id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the account");

    assert_eq!(bound, None, "nothing was written");
}

/// Somebody else's application, which is not confirmed to exist: the answer says there is
/// nothing there and does not say whether there is.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_show_of_something_else_says_nothing_is_there(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;
    const SOMEBODY_ELSE: i64 = 100_000_000_000_000_002;

    support::insert_user(&pool, USER, DISCORD).await;
    let theirs = support::insert_application(&pool, SOMEBODY_ELSE, "ひとつ").await;
    let client_id = support::client_id_of(&pool, theirs).await;

    let response = interaction(
        router(pool),
        application_payload("show", &client_id, DISCORD),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(
        rendered.contains("そのアプリケーションはありません。"),
        "{rendered}"
    );
    assert!(!rendered.contains("ひとつ"), "{rendered}");
}

fn connect_payload(client_id: &str, bot: i64, user: i64) -> Value {
    execute_from_guild(
        json!({
            "name": "application",
            "options": [{
                "name": "connect",
                "type": 1,
                "options": [
                    { "name": "client_id", "type": 3, "value": client_id },
                    { "name": "bot", "type": 6, "value": bot.to_string() },
                ],
            }],
        }),
        user,
    )
}

fn pressed(custom_id: &str, user: i64) -> Value {
    json!({
        "type": 3,
        "data": { "custom_id": custom_id, "component_type": 2 },
        "user": { "id": user.to_string() },
    })
}

/// The control for one field, out of the screen the bot rendered.
///
/// Found by decoding each id and asking which field it names, because all six buttons are
/// labelled 編集 — the label is the same for every field and the id is what differs, so a test
/// that looked for a label would take whichever came first and prove nothing about the field.
fn field_control(screen: &Value, field: &str) -> String {
    fn walk(value: &Value, field: &str, found: &mut Option<String>) {
        match value {
            Value::Object(map) => {
                if let Some(id) = map.get("custom_id").and_then(Value::as_str)
                    && let Ok((_, data)) =
                        vc_api::custom_id::ui::developer::parse(&vc_api::custom_id::parse(id))
                    && vc_api::custom_id::ui::developer::field_of(&data).1 == field
                {
                    *found = Some(id.to_owned());
                }

                for nested in map.values() {
                    walk(nested, field, found);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, field, found);
                }
            }
            _ => {}
        }
    }

    let mut found = None;
    walk(screen, field, &mut found);

    found.unwrap_or_else(|| panic!("no control for {field} in {screen}"))
}

/// 編集 opens a form for the field its button is beside, and that form changes that field and
/// leaves the other eight alone.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_field_button_opens_its_own_form(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "まえ").await;
    let client_id = support::client_id_of(&pool, application).await;

    let shown = interaction(
        router(pool.clone()),
        application_payload("show", &client_id, DISCORD),
    )
    .await;

    assert_eq!(shown.status, 200, "body: {}", shown.body);

    let pressed = interaction(
        router(pool.clone()),
        pressed(&field_control(&shown.body["data"], "client_name"), DISCORD),
    )
    .await;

    assert_eq!(pressed.status, 200, "body: {}", pressed.body);
    assert_eq!(pressed.body["type"], 9, "a form");

    let form = pressed.body["data"]["custom_id"]
        .as_str()
        .expect("a custom_id")
        .to_owned();

    // The form is for this application and this field, which is what the id has to carry: a
    // submission comes back with the id it was opened with and nothing else.
    let (_, data) = vc_api::custom_id::ui::developer::parse(&vc_api::custom_id::parse(&form))
        .expect("a developer screen");
    let (named, which) = vc_api::custom_id::ui::developer::field_of(&data);

    assert_eq!(named, client_id);
    assert_eq!(which, "client_name");

    // It arrives filled in with what the field holds, so an untouched box is what was there.
    let rendered = pressed.body["data"].to_string();

    assert!(
        rendered.contains("まえ"),
        "the value it has now: {rendered}"
    );

    let before = vc_api::routes::oauth2_clients::details(&pool, application)
        .await
        .expect("the application could be read")
        .expect("it exists");

    let submitted = interaction(
        router(pool.clone()),
        submitted_form(
            &form,
            json!([field("クライアント名", "client_name", "あと")]),
            DISCORD,
        ),
    )
    .await;

    assert_eq!(submitted.status, 200, "body: {}", submitted.body);

    let now = vc_api::routes::oauth2_clients::details(&pool, application)
        .await
        .expect("the application could be read")
        .expect("it exists");

    assert_eq!(now.client_name.as_deref(), Some("あと"));
    assert_eq!(
        now.redirect_uris, before.redirect_uris,
        "a field the form did not ask about is untouched, whatever it held"
    );
    assert_eq!(now.application_type, before.application_type);
    assert_eq!(now.grant_types, before.grant_types);
}
