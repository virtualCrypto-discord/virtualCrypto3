//! The screens the DM shows, as data.
//!
//! Each one returns the components of a single message and nothing else: no interaction is
//! parsed here and nothing is fetched, so what a screen says can be tested without
//! Discord and the handlers stay about plumbing.
//!
//! The rules these follow are from the plan in `~/.commandcode/plans/discord-dm-developer-features.md`
//! and from Discord's component reference:
//!
//! - Every screen is one [`container`], because that is what groups a message and gives it
//!   an accent, and the accent is where the state shows.
//! - One Primary button per row; everything else Secondary.
//! - A refusal prints `error_description` **verbatim**. The service answers one sentence
//!   about one state of the world — the bot is not in that server, the integration does
//!   not name this application, that id is not a bot — and each one tells the operator one
//!   thing to fix. Rewriting any of them into a friendlier sentence throws that away.
//! - A list longer than one application is a select, not a row of buttons: a person may
//!   own several, and Discord caps a select at 25 options anyway.

use serde_json::Value;

use crate::components::{
    ButtonStyle, action_row, button, container, link_button, section, select, select_option,
    separator, text, thumbnail,
};

/// The accent each state carries, since the colour is the fastest thing a person reads.
const WORKING: u32 = 0x1ABC9C;
const REFUSED: u32 = 0xED4245;

/// Where a DM starts: what this is, and the two things it is for.
pub fn home(display_name: &str, verification_url: &str) -> Value {
    container(
        Some(WORKING),
        vec![
            text(format!(
                "**VirtualCrypto**\n{display_name} として接続しています。"
            )),
            separator(),
            text("アプリケーションの登録と、そのアプリケーションの Bot の接続ができます。"),
            action_row(vec![
                button("dev:list", "アプリケーション", ButtonStyle::Primary),
                button("dev:connect", "Bot を接続", ButtonStyle::Secondary),
            ]),
            action_row(vec![link_button(verification_url, "この画面について")]),
        ],
    )
}

/// The caller's applications, as a menu.
///
/// Takes what `GET /oauth2/clients` answered. An empty list is not an error and says so
/// with what to do about it, rather than showing an empty menu.
pub fn applications(applications: &[Value]) -> Value {
    if applications.is_empty() {
        return container(
            Some(WORKING),
            vec![
                text("まだアプリケーションを登録していません。"),
                action_row(vec![
                    button("dev:register", "登録する", ButtonStyle::Primary),
                    button("dev:home", "戻る", ButtonStyle::Secondary),
                ]),
            ],
        );
    }

    let options = applications
        .iter()
        .filter_map(|application| {
            let client_id = application["client_id"].as_str()?;
            let name = application["client_name"]
                .as_str()
                .unwrap_or("（名前なし）");

            // The uuid is the value and the name is the label: the label is what somebody
            // typed into a form, and the uuid is what the service will be asked about.
            Some(select_option(client_id, name, Some(client_id)))
        })
        .collect::<Vec<_>>();

    container(
        Some(WORKING),
        vec![
            text(format!(
                "{} 件のアプリケーションがあります。",
                options.len()
            )),
            action_row(vec![select(
                "dev:application",
                "アプリケーションを選ぶ",
                options,
            )]),
            action_row(vec![
                button("dev:register", "登録する", ButtonStyle::Secondary),
                button("dev:home", "戻る", ButtonStyle::Secondary),
            ]),
        ],
    )
}

/// One application, with what can be done to it.
pub fn application(
    client_id: &str,
    name: Option<&str>,
    connected: bool,
    logo_uri: Option<&str>,
    secret: Option<&str>,
) -> Value {
    let name = name.unwrap_or("（名前なし）");
    let state = if connected {
        "Bot が接続されています。"
    } else {
        "Bot はまだ接続されていません。"
    };

    let heading = section(
        vec![text(format!("**{name}**\n{state}\n`{client_id}`"))],
        match logo_uri {
            Some(url) if !url.is_empty() => thumbnail(url),
            _ => button("dev:connect", "接続", ButtonStyle::Primary),
        },
    );

    container(
        Some(if connected { WORKING } else { REFUSED }),
        vec![
            heading,
            separator(),
            // The secret is written on the screen rather than behind a button. The
            // button was mine, not the page's: every response here is ephemeral, so
            // revealing it on request shows it to the person already reading.
            secret_block(secret),
            action_row(vec![
                button("dev:edit", "設定を変更", ButtonStyle::Primary),
                button("dev:connect", "Bot を接続", ButtonStyle::Secondary),
            ]),
            action_row(vec![button("dev:list", "戻る", ButtonStyle::Secondary)]),
        ],
    )
}

/// The secret as it appears on the application's screen.
///
/// The sentence about the `registration_access_token` is the point rather than decoration:
/// the secret can be read here and the token cannot be read anywhere, and the page that
/// hands the token out said the opposite about both until it was checked.
fn secret_block(secret: Option<&str>) -> Value {
    match secret {
        Some(secret) => text(format!(
            "**client_secret**\n```\n{secret}\n```\nこの返信はあなたにだけ見えています。\
             \n`registration_access_token` は登録時にしか表示されず、ここからは読めません。"
        )),
        None => text("client_secret は記録されていません。"),
    }
}

/// What the connect flow answered.
///
/// A refusal carries the service's own sentence through untouched, and the button that
/// opens the form again is the way to act on it.
pub fn connect_result(client_id: &str, refusal: Option<&str>) -> Value {
    match refusal {
        None => container(
            Some(WORKING),
            vec![
                text(
                    "Bot を接続しました。このアプリケーションのアカウントに、Bot の Discord ID が入りました。",
                ),
                action_row(vec![
                    button("dev:application", "アプリケーション", ButtonStyle::Primary),
                    button("dev:home", "戻る", ButtonStyle::Secondary),
                ]),
            ],
        ),
        Some(description) => container(
            Some(REFUSED),
            vec![
                text(format!("接続できませんでした。\n\n{description}")),
                // The form again, because every refusal here is something the operator
                // fixes by entering one of the two ids differently.
                action_row(vec![
                    button("dev:connect", "もう一度", ButtonStyle::Primary),
                    button(
                        "dev:application",
                        "アプリケーション",
                        ButtonStyle::Secondary,
                    ),
                ]),
                text(format!("`{client_id}`")),
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn first_text(value: &Value) -> String {
        value["components"]
            .as_array()
            .expect("components")
            .iter()
            .find(|component| component["type"] == 10)
            .and_then(|component| component["content"].as_str())
            .unwrap_or_default()
            .to_owned()
    }

    /// A refusal is the service's sentence or nothing. If this ever starts reading like a
    /// translation, the thing the operator needed was dropped on the way.
    #[test]
    fn a_refusal_is_shown_verbatim() {
        let description = "VirtualCrypto is not in that server";
        let screen = connect_result("the-client-id", Some(description));

        assert_eq!(
            first_text(&screen),
            format!("接続できませんでした。\n\n{description}")
        );
    }

    #[test]
    fn connecting_says_so_and_offers_the_next_thing() {
        let screen = connect_result("the-client-id", None);

        assert_eq!(screen["accent_color"], WORKING);
        assert!(first_text(&screen).contains("接続しました"));
    }

    /// The menu is the caller's applications, and the uuid is the value rather than the
    /// label — the label is a name somebody typed.
    #[test]
    fn the_menu_offers_each_application_by_its_uuid() {
        let owned = vec![json!({
            "client_id": "3f1c2f4e-9a11-4d2b-8c3e-5f6a7b8c9d0e",
            "client_name": "テスト",
        })];

        let screen = applications(&owned);
        let menu = screen["components"]
            .as_array()
            .expect("components")
            .iter()
            .find(|component| component["type"] == 1)
            .expect("an action row");

        assert_eq!(
            menu["components"][0]["options"][0]["value"],
            "3f1c2f4e-9a11-4d2b-8c3e-5f6a7b8c9d0e"
        );
        assert_eq!(menu["components"][0]["options"][0]["label"], "テスト");
    }

    /// An empty list is a state, not an empty menu: it says what belongs here and what to
    /// do, which is the only useful thing an empty screen can say.
    #[test]
    fn an_empty_list_teaches_the_space() {
        let screen = applications(&[]);

        assert!(first_text(&screen).contains("まだ"));
        assert_eq!(
            screen["components"].as_array().expect("components")[1]["components"][0]["custom_id"],
            "dev:register"
        );
    }

    /// Every button that does something carries an id the dispatcher will recognise, and
    /// nothing carries a `dm_permission`-era guess.
    #[test]
    fn every_button_has_a_custom_id_that_starts_with_dev() {
        fn walk(value: &Value, found: &mut Vec<String>) {
            match value {
                Value::Object(map) => {
                    if map.get("type") == Some(&json!(2))
                        && let Some(id) = map.get("custom_id").and_then(Value::as_str)
                    {
                        found.push(id.to_owned());
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

        let mut found = Vec::new();

        for screen in [
            home("name", "https://example.test/verification"),
            applications(&[]),
            application("id", None, false, None, Some("the-secret")),
            connect_result("id", Some("no")),
            connect_result("id", None),
        ] {
            walk(&screen, &mut found);
        }

        assert!(!found.is_empty());

        for id in found {
            assert!(id.starts_with("dev:"), "{id}");
        }
    }

    /// The screens are components and nothing else, which is what the flag promises; a
    /// stray `content` would be a 400 on the wire.
    #[test]
    fn no_screen_carries_content_or_embeds_at_the_top() {
        for screen in [
            home("name", "https://example.test/verification"),
            applications(&[]),
            application(
                "id",
                None,
                true,
                Some("https://example.test/logo.png"),
                None,
            ),
        ] {
            assert!(screen.get("content").is_none(), "{screen}");
            assert_eq!(screen["type"], 17);
        }
    }
}
