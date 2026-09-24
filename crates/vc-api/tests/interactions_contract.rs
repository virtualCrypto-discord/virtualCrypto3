//! `Interaction.Command.handle/4` for `/contract`, and the buttons its screen
//! carries.
//!
//! An addition with no Elixir counterpart: its contract page is a mockup whose
//! buttons carry no `phx-click`, and it has no Discord surface for contracts at
//! all. What is pinned here is the design — the screen names what the caller was
//! asked for, the buttons are the answers the contract allows, and pressing one
//! locks, refuses or returns money the way the endpoint does.

mod support;

use serde_json::json;
use sqlx::PgPool;
use support::{
    Recorded, Response, button_from_guild, client_id_of, execute_from_guild, fake,
    insert_application, insert_asset, insert_currency, insert_user, interaction,
    state_with_notifier,
};
use vc_api::custom_id::ui::contract::{Action, Page, custom_id, page_custom_id};

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const PARTY: i32 = 2;
const PARTY_DISCORD_ID: i64 = 100_000_000_000_000_001;
const STRANGER: i32 = 3;
const STRANGER_DISCORD_ID: i64 = 100_000_000_000_000_002;
const GUILD: i64 = 900_000_000_000_000_001;

/// An application, a user it is asking, and a balance to lock.
async fn fixture(pool: &PgPool) -> i64 {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, PARTY, PARTY_DISCORD_ID).await;
    insert_user(pool, STRANGER, STRANGER_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, PARTY, 1, 1_000).await;

    insert_application(pool, OWNER_DISCORD_ID, "an application").await
}

/// A contract asking `PARTY` for 100, made through the domain rather than the
/// endpoint: what is under test here is the screen and the buttons.
async fn asking(pool: &PgPool, application: i64, expires_in: Option<i64>) -> i64 {
    vc_core::contract::create(
        pool,
        application,
        "nyan",
        &[vc_core::contract::NewParty {
            discord_id: PARTY_DISCORD_ID,
            amount: 100,
        }],
        None,
        expires_in,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a contract")
}

fn router(pool: &PgPool, recorded: std::sync::Arc<Recorded>) -> axum::Router {
    vc_api::router(state_with_notifier(pool.clone(), fake(), recorded))
}

/// What the one container says, which is everything a screen has to say.
fn texts(response: &Response) -> Vec<String> {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .filter_map(|child| child["content"].as_str().map(str::to_owned))
        .collect()
}

/// The `custom_id` of every button the screen offers, in order.
fn buttons(response: &Response) -> Vec<String> {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .filter(|child| child["type"] == 1)
        .flat_map(|row| {
            row["components"]
                .as_array()
                .expect("an action row's children")
                .iter()
                .filter_map(|component| component["custom_id"].as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        })
        .collect()
}

async fn balance(pool: &PgPool, account: i32) -> i64 {
    sqlx::query_scalar!(
        "SELECT amount FROM assets WHERE user_id = $1 AND currency_id = 1",
        i64::from(account)
    )
    .fetch_optional(pool)
    .await
    .expect("a balance")
    .flatten()
    .unwrap_or(0)
}

/// The screen names who is asking, what the caller's part is, and how far the
/// rest has come — and offers exactly the two answers a pending contract takes.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_shows_what_the_caller_was_asked_for(pool: PgPool) {
    let application = fixture(&pool).await;
    let client_id = client_id_of(&pool, application).await;
    let id = asking(&pool, application, None).await;

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        execute_from_guild(
            json!({ "name": "contract", "options": [{ "name": "list", "type": 1 }] }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let said = texts(&response);
    assert_eq!(
        said[0],
        "**契約** (1件)\n承認すると、その分の通貨がロックされ、アプリケーションが操作できるようになります。"
    );
    assert_eq!(
        said[1],
        format!(
            "Bot未連携: `an application`\nclient_id: `{client_id}`\n\
         （nyan） — 承認待ち\n\
         あなたの分: 100／未回答 ・ 承認 0/1 ・ 残り 0\n\
         送金先: 制限なし\n\
         期限: なし（いつでも取り消せます）"
        )
    );

    assert_eq!(
        buttons(&response),
        [
            custom_id(Action::Approve, id),
            custom_id(Action::Refuse, id),
        ]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn fixed_receivers_and_unrestricted_contracts_are_distinguishable(pool: PgPool) {
    let application = fixture(&pool).await;
    let mut contracts = Vec::new();
    for receiver in [Some(PARTY_DISCORD_ID), Some(STRANGER_DISCORD_ID), None] {
        let id = vc_core::contract::create(
            &pool,
            application,
            "nyan",
            &[vc_core::contract::NewParty {
                discord_id: PARTY_DISCORD_ID,
                amount: 100,
            }],
            receiver,
            None,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
        contracts.push((id, receiver));
    }
    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        execute_from_guild(
            json!({ "name": "contract", "options": [{ "name": "list", "type": 1 }] }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;
    assert_eq!(response.status, 200);
    let children = response.body["data"]["components"][0]["components"]
        .as_array()
        .unwrap();
    for (id, receiver) in contracts {
        let approve = custom_id(Action::Approve, id);
        let row = children
            .iter()
            .position(|child| child["components"][0]["custom_id"].as_str() == Some(&approve))
            .expect("approval button");
        let text = children[row - 1]["content"].as_str().unwrap();
        match receiver {
            Some(receiver) => {
                assert!(text.contains(&format!("送金先: <@{receiver}> に限定")));
                assert!(!text.contains("制限なし"));
            }
            None => {
                assert!(text.contains("送金先: 制限なし"));
                assert!(!text.contains("に限定"));
            }
        }
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn same_named_applications_use_their_own_client_id_or_bound_bot(pool: PgPool) {
    let application = fixture(&pool).await;
    let other = insert_application(&pool, STRANGER_DISCORD_ID, "an application").await;
    asking(&pool, application, None).await;
    asking(&pool, other, None).await;
    let client_id = client_id_of(&pool, application).await;
    let other_client_id = client_id_of(&pool, other).await;
    let recorded = std::sync::Arc::new(Recorded::default());
    let command = execute_from_guild(
        json!({ "name": "contract", "options": [{ "name": "list", "type": 1 }] }),
        PARTY_DISCORD_ID,
    );
    let response = interaction(router(&pool, recorded.clone()), command.clone()).await;
    let said = texts(&response);
    assert!(said[1].contains(&format!("client_id: `{other_client_id}`")));
    assert!(said[2].contains(&format!("client_id: `{client_id}`")));
    assert_ne!(said[1], said[2]);

    let bot = 700_000_000_000_000_001_i64;
    sqlx::query("UPDATE users SET discord_id = $1 WHERE application_id = $2")
        .bind(bot)
        .bind(application)
        .execute(&pool)
        .await
        .unwrap();
    let response = interaction(router(&pool, recorded.clone()), command.clone()).await;
    let said = texts(&response);
    assert!(said[1].starts_with("Bot未連携: `an application`\n"));
    assert!(said[2].starts_with(&format!("<@{bot}>\n")));
    assert!(!said[2].contains("an application"));
    assert!(!said[2].contains(&client_id));
    assert!(!said[2].contains(&OWNER_DISCORD_ID.to_string()));
    assert_eq!(
        response.body["data"]["allowed_mentions"]["parse"],
        json!([])
    );

    // A previous binding must not remain on the consent screen after removal.
    sqlx::query("UPDATE users SET discord_id = NULL WHERE application_id = $1")
        .bind(application)
        .execute(&pool)
        .await
        .unwrap();
    let response = interaction(router(&pool, recorded), command).await;
    assert!(texts(&response)[2].starts_with(&format!(
        "Bot未連携: `an application`\nclient_id: `{client_id}`\n"
    )));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unbound_name_cannot_render_a_bot_mention_or_another_heading(pool: PgPool) {
    let application = fixture(&pool).await;
    sqlx::query("UPDATE applications SET client_name = $1 WHERE id = $2")
        .bind("`\n<@700000000000000001>\r\n**bound bot**")
        .bind(application)
        .execute(&pool)
        .await
        .unwrap();
    asking(&pool, application, None).await;
    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        execute_from_guild(
            json!({ "name": "contract", "options": [{ "name": "list", "type": 1 }] }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;
    assert!(
        texts(&response)[1]
            .starts_with("Bot未連携: `｀ <@700000000000000001>  **bound bot**`\nclient_id: `")
    );
}

/// A user who is named in nothing is told so, and told where such a thing would
/// appear.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_user_who_is_named_in_nothing_says_so(pool: PgPool) {
    let application = fixture(&pool).await;
    asking(&pool, application, None).await;

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        execute_from_guild(
            json!({ "name": "contract", "options": [{ "name": "list", "type": 1 }] }),
            STRANGER_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        [
            "あなたが対象になっている契約はありません。アプリケーションが契約を作ると、\
          ここに承認待ちとして並びます。"
        ]
    );
    assert!(buttons(&response).is_empty());
}

/// Pressing 承認する locks the amount, tells the application, and redraws the
/// screen as the message it came from.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_from_the_button_locks_it(pool: PgPool) {
    let application = fixture(&pool).await;
    let client_id = client_id_of(&pool, application).await;
    let id = asking(&pool, application, None).await;
    let recorded = std::sync::Arc::new(Recorded::default());

    let response = interaction(
        router(&pool, recorded.clone()),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Approve, id) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["type"], 7,
        "the message is updated, not added to"
    );
    assert_eq!(balance(&pool, PARTY).await, 900, "locked");
    assert_eq!(
        recorded.contract_decisions(),
        [(application, id)],
        "and the application is told"
    );

    let said = texts(&response);
    assert_eq!(
        said[1],
        format!(
            "Bot未連携: `an application`\nclient_id: `{client_id}`\n\
         （nyan） — 全員承認済み\n\
         あなたの分: 100／承認済み ・ 承認 1/1 ・ 残り 100\n\
         送金先: 制限なし\n\
         期限: なし（いつでも取り消せます）"
        )
    );
    assert_eq!(
        buttons(&response),
        [custom_id(Action::Withdraw, id)],
        "and the only answer left is taking it back"
    );
}

/// Pressing 拒否する ends the contract, and what was locked comes home.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn refusing_from_the_button_ends_it(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = asking(&pool, application, None).await;

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Refuse, id) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        vc_core::contract::find(&pool, id)
            .await
            .expect("a read")
            .expect("the contract")
            .status,
        "canceled"
    );
    assert_eq!(
        texts(&response),
        [
            "あなたが対象になっている契約はありません。アプリケーションが契約を作ると、\
          ここに承認待ちとして並びます。"
        ],
        "a contract that is over is not shown"
    );
}

/// A button for a contract the presser is not named in is refused in a sentence.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn somebody_elses_contract_is_refused(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = asking(&pool, application, None).await;

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Approve, id) }),
            STRANGER_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        ["エラー: その契約はあなたを対象にしていません。"]
    );
    assert_eq!(balance(&pool, STRANGER).await, 0);
}

/// A temporary contract that is still running offers no 取り消す: the period is
/// what the party agreed to, and a button that would be refused is not shown.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_running_contract_offers_no_withdraw(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = asking(&pool, application, Some(3_600)).await;

    let approved = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Approve, id) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(approved.status, 200, "body: {}", approved.body);
    assert!(buttons(&approved).is_empty(), "{:?}", buttons(&approved));

    // And asking anyway is refused the way the endpoint refuses it.
    let refused = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Withdraw, id) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(refused.status, 200, "body: {}", refused.body);
    assert!(
        texts(&refused)[0].starts_with("エラー: その契約には今この操作ができません。"),
        "{:?}",
        texts(&refused)
    );
    assert_eq!(balance(&pool, PARTY).await, 900, "still locked");
}

/// The command runs where the user is, which is also a DM: nothing about a
/// contract is the guild's to decide.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_answers_in_a_direct_message(pool: PgPool) {
    let application = fixture(&pool).await;
    asking(&pool, application, None).await;

    let payload = json!({
        "type": 2,
        "data": { "name": "contract", "options": [{ "name": "list", "type": 1 }] },
        "user": { "id": PARTY_DISCORD_ID.to_string() },
    });

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        payload,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response).len(), 2, "the screen is the same one");
}

/// The screen shows five, says how many there are, and reaches the rest: the count
/// and the page cost one bounded read each, where saying the number used to cost
/// reading every contract the caller is named in — with every one's parties.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_sixth_contract_is_a_page_and_a_count(pool: PgPool) {
    let application = fixture(&pool).await;

    let mut ids = Vec::new();
    for _ in 0..7 {
        ids.push(asking(&pool, application, None).await);
    }

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        execute_from_guild(
            json!({ "name": "contract", "options": [{ "name": "list", "type": 1 }] }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let said = texts(&response);
    assert_eq!(
        said[0],
        "**契約** (7件)\n承認すると、その分の通貨がロックされ、アプリケーションが操作できるようになります。"
    );

    // Five shown, newest first, and each with the two answers a pending contract
    // takes: the page is the newest five, not any five.
    let offered = buttons(&response);
    assert_eq!(
        offered[0..2],
        [
            custom_id(Action::Approve, ids[6]),
            custom_id(Action::Refuse, ids[6]),
        ]
    );

    // And the four arrows, with the two that have nowhere to go disabled: there is
    // a second page, which is where the sixth and seventh contracts are.
    assert!(
        offered.contains(&page_custom_id(Page::Next, 2)),
        "{offered:?}"
    );
    assert!(
        offered.contains(&page_custom_id(Page::Last, 2)),
        "{offered:?}"
    );
    assert!(offered.contains(&"disabled-0".to_owned()), "{offered:?}");
    assert!(offered.contains(&"disabled-1".to_owned()), "{offered:?}");
}

/// The arrows are how the rest of the list is reached — which "答えると一覧が進み
/// ます" could not do: a contract that is approved and running cannot be answered
/// and cannot be withdrawn, so five of those would have hidden every one behind
/// them with nothing to press.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_arrows_reach_the_rest_of_the_list(pool: PgPool) {
    let application = fixture(&pool).await;

    let mut ids = Vec::new();
    for _ in 0..7 {
        ids.push(asking(&pool, application, None).await);
    }

    let second = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": page_custom_id(Page::Next, 2) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(second.status, 200, "body: {}", second.body);
    assert_eq!(
        second.body["type"], 7,
        "the message is updated, not added to"
    );
    assert_eq!(
        texts(&second)[0],
        "**契約** (7件)\n承認すると、その分の通貨がロックされ、アプリケーションが操作できるようになります。",
        "the count is the list's, not the page's"
    );

    // The two oldest, newest first, and the way back.
    let offered = buttons(&second);
    assert_eq!(
        offered[0..2],
        [
            custom_id(Action::Approve, ids[1]),
            custom_id(Action::Refuse, ids[1]),
        ]
    );
    assert!(offered.contains(&custom_id(Action::Approve, ids[0])));
    assert!(
        offered.contains(&page_custom_id(Page::First, 1)),
        "{offered:?}"
    );
    assert!(
        offered.contains(&page_custom_id(Page::Previous, 1)),
        "{offered:?}"
    );
    assert!(offered.contains(&"disabled-2".to_owned()), "{offered:?}");
    assert!(offered.contains(&"disabled-3".to_owned()), "{offered:?}");

    // Every arrow, pressed, not just offered: the four are one call with the page in the id,
    // so what a press has to show is the page it named. ⏪ and ⏮️ go back to the newest five.
    let arrows = &offered[4..];

    for arrow in [&arrows[0], &arrows[1]] {
        let back = interaction(
            router(&pool, std::sync::Arc::new(Recorded::default())),
            button_from_guild(json!({ "custom_id": arrow }), PARTY_DISCORD_ID),
        )
        .await;

        assert_eq!(back.status, 200, "body: {}", back.body);
        assert_eq!(back.body["type"], 7, "the message is updated, not added to");
        assert_eq!(
            texts(&back)[0],
            "**契約** (7件)\n承認すると、その分の通貨がロックされ、アプリケーションが操作できるようになります。"
        );

        let offered = buttons(&back);
        assert_eq!(
            offered[0..2],
            [
                custom_id(Action::Approve, ids[6]),
                custom_id(Action::Refuse, ids[6]),
            ],
            "the newest five again"
        );
        assert!(
            offered.contains(&page_custom_id(Page::Next, 2)),
            "and ⏭️ has somewhere to go again: {offered:?}"
        );
    }

    // And ⏩ — the id the first page carries, which the page above found among its buttons —
    // which is the end of the list, where ⏭️ went.
    let last = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": page_custom_id(Page::Last, 2) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(last.status, 200, "body: {}", last.body);
    assert_eq!(last.body["type"], 7);

    let offered = buttons(&last);
    assert_eq!(
        offered[0..2],
        [
            custom_id(Action::Approve, ids[1]),
            custom_id(Action::Refuse, ids[1]),
        ]
    );
    assert!(offered.contains(&custom_id(Action::Approve, ids[0])));
    assert!(offered.contains(&"disabled-3".to_owned()), "{offered:?}");
}
