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

/// `/help`: every command, the sentence Discord's picker shows for it, and the
/// menu that opens one.
///
/// The list is the registration payload's, not a copy of it: a command that is
/// registered and missing here would be a command nobody can find out about.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn help(pool: PgPool) {
    let response = interaction(
        router(pool),
        execute_from_guild(json!({ "name": "help" }), 12),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], 4, "a message: {}", response.body);
    assert_eq!(response.body["data"]["flags"], json!(32832));

    let rendered = response.body["data"].to_string();

    for command in vc_api::discord_commands::commands() {
        let name = command["name"].as_str().expect("a name");
        let description = command["description"].as_str().expect("a description");

        assert!(
            rendered.contains(&format!("**/{name}**")),
            "{name} is not in {rendered}"
        );
        assert!(
            rendered.contains(description),
            "what Discord says about {name} is not in {rendered}"
        );
    }

    // The four that ask for the administrator bit say so, which is read off the
    // registration payload rather than written down a second time.
    assert!(rendered.contains("**/issue**（管理者）"), "{rendered}");

    // The list carries the project's logo as its accessory. That the URL behind it answers
    // with a picture rather than a 404 is `tests/web.rs`'s business — this is the screen that
    // shows it, so it is the screen that says the URL.
    assert!(
        rendered.contains(&format!("\"url\":\"{SITE_URL}/static/images/logo.jpg\"")),
        "{rendered}"
    );

    // The menu carries the id the dispatcher reads, and the addresses are the
    // deployment's.
    assert_eq!(
        select_id(&response.body["data"]),
        vc_api::custom_id::ui::help::select()
    );
    assert!(
        rendered.contains(&format!("{SITE_URL}/document/commands")),
        "{rendered}"
    );
    assert!(rendered.contains(BOT_INVITE_URL), "{rendered}");
    assert!(rendered.contains(SUPPORT_GUILD_INVITE_URL), "{rendered}");
}

/// `/help command:<名前>`: the same screen the menu opens, reached by typing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn help_opens_one_command(pool: PgPool) {
    let response = interaction(
        router(pool),
        execute_from_guild(
            json!({
                "name": "help",
                "options": [{ "name": "command", "type": 3, "value": "pay" }],
            }),
            12,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(rendered.contains("**/pay**"), "{rendered}");
    assert!(rendered.contains("## 使い方"), "{rendered}");
    assert!(
        rendered.contains("/pay unit:<通貨の単位> user:<送信先> amount:<枚数>"),
        "{rendered}"
    );

    // The options are the registered ones, requiredness and all.
    assert!(rendered.contains("送信先のユーザーです。"), "{rendered}");
    assert!(rendered.contains("（必須）"), "{rendered}");
}

/// A name that is not a command is answered with the list and one sentence about
/// it: the menu offers the real ones, and a typed value can be anything.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn help_says_when_a_name_is_not_a_command(pool: PgPool) {
    let response = interaction(
        router(pool),
        execute_from_guild(
            json!({
                "name": "help",
                "options": [{ "name": "command", "type": 3, "value": "nope" }],
            }),
            12,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(
        rendered.contains("`nope` というコマンドはありません。"),
        "{rendered}"
    );
    assert!(rendered.contains("**/bal**"), "and the list: {rendered}");
}

/// A choice from the menu redraws the message it was made on: the screen is one
/// message a person moves through, and a message per choice would leave a trail.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_menu_opens_the_command_it_names(pool: PgPool) {
    let chosen = interaction(
        router(pool),
        chose_multi(&vc_api::custom_id::ui::help::select(), &["claim"], 12),
    )
    .await;

    assert_eq!(chosen.status, 200, "body: {}", chosen.body);
    assert_eq!(
        chosen.body["type"], 7,
        "a change to the message: {}",
        chosen.body
    );

    let rendered = chosen.body["data"].to_string();

    assert!(rendered.contains("**/claim**"), "{rendered}");
    assert!(rendered.contains("/claim make user:<請求先>"), "{rendered}");
}

/// And the button goes back to the list, which is where somebody who opened the
/// wrong command wants to be.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_back_button_shows_the_list_again(pool: PgPool) {
    let pressed = interaction(
        router(pool),
        pressed(&vc_api::custom_id::ui::help::index(), 12),
    )
    .await;

    assert_eq!(pressed.status, 200, "body: {}", pressed.body);
    assert_eq!(pressed.body["type"], 7);

    let rendered = pressed.body["data"].to_string();

    assert!(rendered.contains("## コマンド"), "{rendered}");
    assert!(rendered.contains("**/bal**"), "{rendered}");
}

/// `/help` sends somebody who has just been handed the bot to 「はじめに」 — the page itself,
/// rendered here, rather than a jump to the site.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn help_opens_the_starting_page(pool: PgPool) {
    let response = interaction(
        router(pool.clone()),
        execute_from_guild(json!({ "name": "help" }), 12),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert!(
        response.body["data"].to_string().contains(&format!(
            "\"custom_id\":\"{}\"",
            vc_api::custom_id::ui::help::start()
        )),
        "the way in: {}",
        response.body
    );

    // Pressing it draws the page, which is the document the site shows rather than a link to it.
    let pressed = interaction(
        router(pool),
        support::button_from_guild(
            json!({ "custom_id": vc_api::custom_id::ui::help::start() }),
            12,
        ),
    )
    .await;

    assert_eq!(pressed.status, 200, "body: {}", pressed.body);

    let rendered = pressed.body["data"].to_string();

    assert!(rendered.contains("はじめに"), "{rendered}");
    assert!(
        rendered.contains("独自の通貨を使えるようにするBotです"),
        "the page's own prose: {rendered}"
    );
}

/// A command is a mention — `</name:id>`, which Discord makes pressable — when the
/// application's ids are known, and the plain name when they are not.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn help_links_a_command_when_discord_knows_its_id(pool: PgPool) {
    let response = interaction(
        vc_api::router(support::state(
            pool,
            support::FakeDiscord::with_commands(&[("bal", 1_234_567_890)]),
        )),
        execute_from_guild(json!({ "name": "help" }), 12),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(rendered.contains("</bal:1234567890>"), "{rendered}");
    assert!(
        !rendered.contains("**/bal**"),
        "the mention replaces the bold name: {rendered}"
    );
}

/// And 「はじめに」, which is the same document the site renders: every command it names that
/// Discord has an id for is a link there, and the one it does not stays the code it was
/// written as.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_starting_page_links_the_commands_it_names(pool: PgPool) {
    let response = interaction(
        vc_api::router(support::state(
            pool,
            support::FakeDiscord::with_commands(&[("create", 11), ("pay", 22), ("bal", 33)]),
        )),
        support::button_from_guild(
            json!({ "custom_id": vc_api::custom_id::ui::help::start() }),
            12,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(
        rendered.contains("</create:11> サーバーに通貨を作る"),
        "the list of what this does: {rendered}"
    );
    assert!(
        rendered.contains("</pay:22> で配る。</bal:33> で自分の残高を確認できます。"),
        "two commands in one sentence: {rendered}"
    );
    assert!(
        rendered.contains("`/info` で総発行量"),
        "a command with no id here is left as it was written: {rendered}"
    );
}

/// A usage line is inside a fence — what somebody types — and Discord does not resolve a
/// mention there: it shows the raw `</claim make:22>` rather than `/claim make`. So the line
/// stays exactly as it was written, whether or not this deployment has an id for it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_usage_line_stays_as_it_was_written(pool: PgPool) {
    let response = interaction(
        vc_api::router(support::state(
            pool,
            support::FakeDiscord::with_command_payloads(vec![json!({
                "name": "claim",
                "id": "21",
                "options": [
                    { "name": "make", "id": "22", "type": 1 },
                    { "name": "show", "id": "23", "type": 1 },
                ],
            })]),
        )),
        execute_from_guild(
            json!({
                "name": "help",
                "options": [{ "name": "command", "type": 3, "value": "claim" }],
            }),
            12,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(
        rendered.contains("/claim make user:<請求先> unit:<通貨の単位> amount:<枚数>"),
        "the usage line, as it was written: {rendered}"
    );
    assert!(rendered.contains("/claim show id:<請求番号>"), "{rendered}");

    let (in_code, _) = code_mentions(&response.body["data"]);

    assert!(
        in_code.is_empty(),
        "an id for the subcommand does not put a mention in its usage fence: {in_code:?}"
    );
}

/// Nothing puts a mention inside a code block or an inline code span, anywhere in a screen.
///
/// A mention is a link in prose and raw markup in code — Discord does not resolve one inside a
/// fence or an inline span, so the person reads `</pat:1551719234256637972>` instead of `/pat`.
/// Every command's own screen is rendered here, with an id for every command and every
/// subcommand, which is the state that makes a mention possible at all, and the code is read
/// back out of each answer rather than off the source: a fence that ate a mention is a fence
/// this sees.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn no_command_puts_a_mention_in_a_code_block(pool: PgPool) {
    let discord = support::FakeDiscord::with_command_payloads(commands_with_ids());

    // The list first: it is the same walk over the same text, and it is where a person starts.
    let menu = interaction(
        vc_api::router(support::state(pool.clone(), discord.clone())),
        execute_from_guild(json!({ "name": "help" }), 12),
    )
    .await;

    assert_eq!(menu.status, 200, "body: {}", menu.body);

    let (in_code, _) = code_mentions(&menu.body["data"]);

    assert!(in_code.is_empty(), "the command list: {in_code:?}");

    let mut mentions_in_prose = 0;

    for showing in vc_api::docs::showings() {
        let response = interaction(
            vc_api::router(support::state(pool.clone(), discord.clone())),
            execute_from_guild(
                json!({
                    "name": "help",
                    "options": [{ "name": "command", "type": 3, "value": showing.name }],
                }),
                12,
            ),
        )
        .await;

        assert_eq!(response.status, 200, "body: {}", response.body);

        // The usage fence holds what was written, not what a mention would replace it with.
        let usage = showing.usage[0];

        assert!(
            response.body["data"].to_string().contains(usage),
            "the usage fence of {} does not hold {usage:?}: {}",
            showing.name,
            response.body["data"]
        );

        let (in_code, in_prose) = code_mentions(&response.body["data"]);

        assert!(
            in_code.is_empty(),
            "a mention is raw markup in a code block of {}: {in_code:?}",
            showing.name
        );

        mentions_in_prose += in_prose;
    }

    // The ids above are known, so the prose does name commands by mention. Without this the
    // check above would pass on a screen that never had a mention to put anywhere.
    assert!(
        mentions_in_prose > 0,
        "the prose holds no mention, so nothing here proved anything"
    );
}

/// Every registered command with an id, and an id for each of its subcommands — what Discord
/// answers `get_application_commands` with once the commands are registered.
fn commands_with_ids() -> Vec<Value> {
    vc_api::discord_commands::commands()
        .iter()
        .enumerate()
        .map(|(index, command)| {
            let base = 1_000_000_000 + index as u64 * 100;

            let subcommands: Vec<Value> = command["options"]
                .as_array()
                .into_iter()
                .flatten()
                // 1 is a subcommand and 2 a group of them; both take a place in the path.
                .filter(|option| option["type"] == 1 || option["type"] == 2)
                .enumerate()
                .map(|(sub, option)| {
                    json!({
                        "name": option["name"].clone(),
                        "id": (base + sub as u64 + 1).to_string(),
                        "type": 1,
                    })
                })
                .collect();

            let mut payload = json!({
                "name": command["name"].clone(),
                "id": base.to_string(),
            });

            if !subcommands.is_empty() {
                payload["options"] = Value::Array(subcommands);
            }

            payload
        })
        .collect()
}

/// Every string a rendered answer carries.
fn strings(value: &Value, into: &mut Vec<String>) {
    match value {
        Value::String(text) => into.push(text.clone()),
        Value::Object(map) => map.values().for_each(|nested| strings(nested, into)),
        Value::Array(items) => items.iter().for_each(|nested| strings(nested, into)),
        _ => {}
    }
}

/// One rendered string taken apart the way Discord reads it: the prose it renders, and the code
/// it does not — a fence is what a pair of ``` holds, and an inline span what a pair of ` holds.
fn prose_and_code(text: &str) -> (Vec<&str>, Vec<&str>) {
    let mut prose = Vec::new();
    let mut code = Vec::new();

    for (at, block) in text.split("```").enumerate() {
        if at % 2 == 1 {
            code.push(block);
            continue;
        }

        for (at, span) in block.split('`').enumerate() {
            if at % 2 == 1 {
                code.push(span);
            } else {
                prose.push(span);
            }
        }
    }

    (prose, code)
}

/// The code pieces of a rendered answer that hold a command mention — which is none of them —
/// and how many mentions the prose around that code holds.
fn code_mentions(data: &Value) -> (Vec<String>, usize) {
    let mut rendered = Vec::new();
    strings(data, &mut rendered);

    let mut in_code = Vec::new();
    let mut in_prose = 0;

    for text in &rendered {
        let (prose, code) = prose_and_code(text);

        for piece in code {
            if piece.contains("</") {
                in_code.push(piece.to_owned());
            }
        }

        in_prose += prose.iter().filter(|piece| piece.contains("</")).count();
    }

    (in_code, in_prose)
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
        response.body["data"]["components"],
        json!([{
            "type": 17,
            "accent_color": 0x0062_21ED,
            "components": [
                {
                    "type": 9,
                    "components": [{ "type": 10, "content": "**VirtualCrypto**" }],
                    "accessory": {
                        "type": 11,
                        "media": { "url": format!("{SITE_URL}/static/images/logo.jpg") },
                    },
                },
                { "type": 10, "content": description },
            ],
        }])
    );
    assert_eq!(response.body["data"]["flags"], json!(32832));
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

/// The screen carries the subscription menu: the two events as choices, and the
/// menu's id names the application and the field, the way the other menus do.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_show_offers_the_event_menu(pool: PgPool) {
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

    let rendered = response.body["data"].to_string();

    assert!(rendered.contains("通知イベント"), "{rendered}");
    assert!(rendered.contains("subscribed_events"), "{rendered}");
}

/// Choosing the events writes the set and shows the screen again, so the person
/// sees the set they now have rather than the one they chose.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn choosing_events_writes_the_set(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;

    let shown = interaction(
        router(pool.clone()),
        application_payload("show", &client_id, DISCORD),
    )
    .await;

    assert_eq!(shown.status, 200, "body: {}", shown.body);

    let menu = field_menu(&shown.body["data"], "subscribed_events");

    let response = interaction(router(pool.clone()), chose_multi(&menu, &["3"], DISCORD)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let now = vc_api::routes::oauth2_clients::details(&pool, application)
        .await
        .expect("the application could be read")
        .expect("it exists");

    assert_eq!(now.subscribed_events, [3]);

    let rendered = response.body["data"].to_string();

    assert!(rendered.contains("発行許可の決定"), "{rendered}");
}

/// The menu for one set field, out of the screen the bot rendered: the id whose
/// field half names it.
fn field_menu(screen: &Value, field: &str) -> String {
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

    found.unwrap_or_else(|| panic!("no menu for {field} in {screen}"))
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

fn field(label: &str, custom_id: &str, value: &str) -> Value {
    json!({
        "type": 18,
        "label": label,
        "component": { "type": 4, "custom_id": custom_id, "value": value },
    })
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
    chose_multi(custom_id, &[value], user)
}

/// The same choice with several values, which is what a set menu sends: the
/// values are the set the person chose, and the set is what is stored.
fn chose_multi(custom_id: &str, values: &[&str], user: i64) -> Value {
    json!({
        "type": 3,
        "data": {
            "custom_id": custom_id,
            "component_type": 3,
            "values": values,
        },
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
/// and the proof is Discord's — the guild's integration for this bot carries the token the
/// application's own screen shows.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_connect_in_a_guild_binds_the_bot(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;
    const BOT: i64 = 200_000_000_000_000_002;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;
    let described = format!(
        "{}/applications/verification?q={client_id}",
        support::links().site_url
    );

    let discord = Arc::new(support::FakeDiscord::with_integrations(
        json!({ "name": "TestGuild" }),
        &[(BOT, &described)],
    ));

    let response = interaction(
        vc_api::router(support::state(pool.clone(), discord)),
        chose_bot(&client_id, BOT, DISCORD),
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

/// A bot whose integration does not carry this application's token, and the answer is the
/// service's own sentence about it — which is the thing a person has to act on, unchanged.
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
        chose_bot(&client_id, BOT, DISCORD),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(
        rendered
            .contains("the integration's description does not contain this application's token"),
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

/// Choosing a bot from the menu on an application's screen, which is the only way to connect
/// now: the subcommand that took a bot as an option is gone, and the guild comes from the
/// interaction rather than from anything typed.
fn chose_bot(client_id: &str, bot: i64, user: i64) -> Value {
    json!({
        "type": 3,
        "data": {
            "custom_id": vc_api::custom_id::ui::developer::custom_id_for(
                vc_api::custom_id::ui::developer::Screen::Connect,
                client_id,
            ),
            "component_type": 5,
            "values": [bot.to_string()],
        },
        "member": { "user": { "id": user.to_string() } },
        "guild_id": "100000000000000002",
    })
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

/// `/application register` asks nothing: every field has a default, so an application is made
/// with them and answered with the screen that changes them.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_register_makes_one_with_defaults(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    support::insert_user(&pool, USER, DISCORD).await;

    let response = interaction(
        router(pool.clone()),
        execute_from_guild(
            json!({ "name": "application", "options": [{ "name": "register", "type": 1 }] }),
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

    // The defaults, including the two that are choices: a grant type, because none makes an
    // application that can do nothing, and no webhook, because a value there would handshake
    // against somebody else's server while the registration runs.
    assert_eq!(found.application_type, "web");
    assert_eq!(found.grant_types, vec!["authorization_code".to_owned()]);
    assert_eq!(found.response_types, vec!["code".to_owned()]);
    assert!(found.redirect_uris.is_empty());
    assert!(found.webhook_url.is_none(), "{:?}", found.webhook_url);

    // And the answer is the screen, with the secret on it, because the next thing is setting
    // what the defaults could not know.
    let rendered = response.body["data"].to_string();

    assert!(rendered.contains(&found.client_id), "{rendered}");
    assert!(
        rendered.contains(found.client_secret.as_deref().expect("a secret")),
        "{rendered}"
    );

    field_control(&response.body["data"], "client_name");
    field_control(&response.body["data"], "application_type");
}

/// A DM has no guild to connect to, and the picker says where to go rather than failing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bot_picked_in_a_dm_says_where_to_run_it(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;
    const BOT: i64 = 200_000_000_000_000_002;

    support::insert_user(&pool, USER, DISCORD).await;
    let application = support::insert_application(&pool, DISCORD, "テスト").await;
    let client_id = support::client_id_of(&pool, application).await;

    // The same choice, made where there is no guild: `chose_bot` builds the interaction from a
    // guild, and what a DM lacks is exactly that.
    let mut payload = chose_bot(&client_id, BOT, DISCORD);
    payload
        .as_object_mut()
        .expect("an object")
        .remove("guild_id");

    let response = interaction(router(pool), payload).await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rendered = response.body["data"].to_string();

    assert!(
        rendered.contains("サーバーの中で行います"),
        "it says where to run it: {rendered}"
    );

    // And what it names is a subcommand that exists: the one this replaced was deleted with the
    // rest of the connect command.
    assert!(rendered.contains("`/application show`"), "{rendered}");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn optional_application_fields_can_be_cleared_through_the_modal(pool: PgPool) {
    use vc_api::custom_id::ui::developer::{Screen, custom_id_for_field};
    let owner = 500_000_000_000_000_001;
    support::insert_user(&pool, 1, owner).await;
    let application = support::insert_application(&pool, owner, "mine").await;
    sqlx::query(
        "UPDATE applications SET webhook_url = 'https://example.test/webhook' WHERE id = $1",
    )
    .bind(application)
    .execute(&pool)
    .await
    .unwrap();
    let client_id = support::client_id_of(&pool, application).await;
    for name in [
        "client_name",
        "redirect_uris",
        "client_uri",
        "logo_uri",
        "webhook_url",
        "discord_support_server_invite_slug",
    ] {
        let custom_id = custom_id_for_field(Screen::Edit, &client_id, name);
        let payload = json!({"type":3,"user":{"id":owner.to_string()},"data":{"component_type":2,"custom_id":custom_id}});
        let response = interaction(router(pool.clone()), payload).await;
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(response.body["type"], 9);
        assert_eq!(
            response.body["data"]["components"][0]["component"]["required"], false,
            "{name}"
        );
    }
    let custom_id = custom_id_for_field(Screen::Edit, &client_id, "webhook_url");
    let response = interaction(
        router(pool.clone()),
        submitted_form(
            &custom_id,
            json!([field("webhook URL", "webhook_url", "")]),
            owner,
        ),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body);
    let details = vc_api::routes::oauth2_clients::details(&pool, application)
        .await
        .unwrap()
        .unwrap();
    assert!(details.webhook_url.is_none());
}
