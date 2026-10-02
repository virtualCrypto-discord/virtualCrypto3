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
//! - A screen's buttons move or choose; they do not open forms. A form belongs to a
//!   subcommand — where the fields that are free text are in the command list and autocomplete
//!   fills the ones that name something — or it is a control on the screen, which is where the
//!   enumerated fields are set and where a bot is chosen.
//! - A button that goes nowhere is not on a screen. 戻る named `Home`, which nothing renders
//!   and nothing dispatches, so pressing it failed.
//! - One Primary button per row; everything else Secondary.
//! - A refusal prints `error_description` **verbatim**. The service answers one sentence
//!   about one state of the world — the bot is not in that server, the integration does
//!   not name this application, that id is not a bot — and each one tells the operator one
//!   thing to fix. Rewriting any of them into a friendlier sentence throws that away.
//! - A list longer than one application is a select, not a row of buttons: a person may
//!   own several, and Discord caps a select at 25 options anyway.

use std::collections::BTreeMap;

use serde_json::Value;

use vc_core::application::{APPLICATION_TYPES, EVENT_TYPES, GRANT_TYPES, RESPONSE_TYPES};

use crate::components::{
    ButtonStyle, action_row, button, container, icon_button, section, select, select_many,
    select_option, text, thumbnail, user_select,
};

/// The accent each state carries, since the colour is the fastest thing a person reads.
const WORKING: u32 = 0x1ABC9C;
const REFUSED: u32 = 0xED4245;

/// The caller's applications, as a menu.
///
/// Takes what `GET /oauth2/clients` answered. An empty list is not an error and says so
/// with what to do about it, rather than showing an empty menu.
pub fn applications(applications: &[Value], ids: &BTreeMap<String, u64>) -> Value {
    applications_page(
        &applications[..applications.len().min(APPLICATIONS_PER_PAGE)],
        1,
        applications.len(),
        ids,
    )
}

pub const APPLICATIONS_PER_PAGE: usize = 25;

/// One page of the caller's applications, with the same four arrows as the balance list.
pub fn applications_page(
    applications: &[Value],
    page: usize,
    total: usize,
    ids: &BTreeMap<String, u64>,
) -> Value {
    if applications.is_empty() {
        return container(
            Some(WORKING),
            vec![text(crate::docs::discord::mentions(
                message!("developer.applications_page.001"),
                ids,
            ))],
        );
    }

    let options = applications
        .iter()
        .filter_map(|application| {
            let client_id = application["client_id"].as_str()?;
            let name = application["client_name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or(message!("developer.applications_page.002"));

            // The uuid is the value and the name is the label: the label is what somebody
            // typed into a form, and the uuid is what the service will be asked about.
            Some(select_option(client_id, name, Some(client_id)))
        })
        .collect::<Vec<_>>();

    let mut children = vec![
        text(format!(
            message!("developer.applications_page.003"),
            total = total
        )),
        action_row(vec![select(
            &crate::custom_id::ui::developer::custom_id(
                crate::custom_id::ui::developer::Screen::Back,
            ),
            message!("developer.applications_page.004"),
            options,
        )]),
    ];
    let last = total.div_ceil(APPLICATIONS_PER_PAGE).max(1);
    if last > 1 {
        use crate::custom_id::ui::developer::{Screen, custom_id_for_field};
        let arrow = |direction: &str, emoji: &str, target: usize, disabled: bool| {
            icon_button(
                &custom_id_for_field(Screen::List, &target.to_string(), direction),
                emoji,
                ButtonStyle::Secondary,
                Some(disabled),
            )
        };
        children.push(action_row(vec![
            arrow("first", "⏪", 1, page == 1),
            arrow("previous", "⏮️", page.saturating_sub(1).max(1), page == 1),
            arrow("next", "⏭️", page.saturating_add(1).min(last), page == last),
            arrow("last", "⏩", last, page == last),
        ]));
    }
    container(Some(WORKING), children)
}

/// What an application is now, field by field: the nine the endpoint lets an edit change,
/// and the events its webhook wants.
///
/// They travel together because the screen shows all nine and each has its own control, and
/// because a screen that names each as its own argument is a signature nobody reads.
pub struct Fields<'a> {
    pub client_name: Option<&'a str>,
    pub redirect_uris: &'a [String],
    pub client_uri: Option<&'a str>,
    pub logo_uri: Option<&'a str>,
    pub webhook_url: Option<&'a str>,
    pub discord_support_server_invite_slug: Option<&'a str>,
    pub application_type: &'a str,
    pub grant_types: &'a [String],
    pub response_types: &'a [String],
    /// The `type` values the application wants delivered. Checked is sent and
    /// unchecked is not, and the menu below sends what is checked — so what the
    /// screen shows and what the row holds are the same set.
    pub subscribed_events: &'a [i64],
}

/// One application, with what can be done to it.
///
/// The bot identity is informational; an unconnected application is not an error.
pub fn application(
    client_id: &str,
    name: Option<&str>,
    connected_bot: Option<i64>,
    logo_uri: Option<&str>,
    secret: Option<&str>,
    token: &str,
    fields: Fields<'_>,
) -> Value {
    let name = name.unwrap_or(message!("developer.application.001"));
    let state = match connected_bot {
        Some(bot) => format!(message!("developer.application.002"), bot = bot),
        None => message!("developer.application.003").to_owned(),
    };

    let title = format!("**{name}**\n{state}\n`{client_id}`");
    let heading = match logo_uri {
        Some(url) if !url.is_empty() => section(vec![text(&title)], thumbnail(url)),
        // Without a logo a section has nothing for its accessory, and the button that was there
        // went to the connect screen with no bot to connect with.
        _ => text(&title),
    };

    let mut children = vec![heading];

    children.extend([
        // The six fields that are free text, each beside the value it edits and with its own
        // button — a text input is a component only a modal may carry, so these are the six
        // that need a form, and the form asks for exactly the field its button is next to.
        // A section is what puts a control beside the text it is about; none of them is
        // gathered at the bottom, because a button away from its value is a guess.
        editing(
            client_id,
            "client_name",
            message!("developer.application.004"),
            fields.client_name,
        ),
        editing(
            client_id,
            "redirect_uris",
            message!("developer.application.005"),
            Some(&listing(fields.redirect_uris)),
        ),
        editing(
            client_id,
            "client_uri",
            message!("developer.application.006"),
            fields.client_uri,
        ),
        editing(
            client_id,
            "logo_uri",
            message!("developer.application.007"),
            fields.logo_uri,
        ),
        editing(
            client_id,
            "webhook_url",
            message!("common.webhookUrl"),
            fields.webhook_url,
        ),
        editing(
            client_id,
            "discord_support_server_invite_slug",
            message!("developer.application.008"),
            fields.discord_support_server_invite_slug,
        ),
        // The three fields whose values are a set the service defines, each with its own
        // menu and nothing behind a button: a string select is the one select a message may
        // carry with options we choose, which is exactly what an enumerated field is, and
        // the set that comes back is the set that is sent.
        text(message!("developer.application.009")),
        action_row(vec![select_many(
            &crate::custom_id::ui::developer::custom_id_for_field(
                crate::custom_id::ui::developer::Screen::Edit,
                client_id,
                "application_type",
            ),
            Some(message!("developer.application.010")),
            APPLICATION_TYPES
                .iter()
                .map(|kind| setting_option(kind, kind, *kind == fields.application_type))
                .collect(),
            1,
            1,
        )]),
        text(message!("developer.application.011")),
        action_row(vec![select_many(
            &crate::custom_id::ui::developer::custom_id_for_field(
                crate::custom_id::ui::developer::Screen::Edit,
                client_id,
                "grant_types",
            ),
            Some(message!("developer.application.012")),
            GRANT_TYPES
                .iter()
                .map(|kind| {
                    setting_option(
                        kind,
                        kind,
                        fields.grant_types.iter().any(|value| value == *kind),
                    )
                })
                .collect(),
            0,
            GRANT_TYPES.len() as u8,
        )]),
        text(message!("developer.application.013")),
        action_row(vec![select_many(
            &crate::custom_id::ui::developer::custom_id_for_field(
                crate::custom_id::ui::developer::Screen::Edit,
                client_id,
                "response_types",
            ),
            Some(message!("developer.application.014")),
            RESPONSE_TYPES
                .iter()
                .map(|kind| {
                    setting_option(
                        kind,
                        kind,
                        fields.response_types.iter().any(|value| value == *kind),
                    )
                })
                .collect(),
            0,
            RESPONSE_TYPES.len() as u8,
        )]),
        // The events the webhook wants, as the `type` values the deliveries
        // carry: 2 for a claim update, 3 for a grant decision, 4 for a contract decision. A string
        // select is the one select a message may carry with options we
        // choose, which is exactly what an enumerated field is — the same
        // shape as the two menus above, and for the same reason. The set
        // that comes back is the set that is stored: checked is sent,
        // unchecked is not, and empty is nothing — which needs no
        // enforcement, because Discord lets the menu come back empty.
        text(message!("developer.application.015")),
        action_row(vec![select_many(
            &crate::custom_id::ui::developer::custom_id_for_field(
                crate::custom_id::ui::developer::Screen::Edit,
                client_id,
                "subscribed_events",
            ),
            Some(message!("developer.application.016")),
            EVENT_TYPES
                .iter()
                .map(|kind| {
                    setting_option(
                        &kind.to_string(),
                        event_name(*kind),
                        fields.subscribed_events.contains(kind),
                    )
                })
                .collect(),
            0,
            EVENT_TYPES.len() as u8,
        )]),
        secret_block(secret),
        action_row(vec![button(
            &crate::custom_id::ui::developer::custom_id_for(
                crate::custom_id::ui::developer::Screen::RotateSecret,
                client_id,
            ),
            message!("developer.application.017"),
            ButtonStyle::Secondary,
        )]),
        text(format!(
            message!("developer.application.018"),
            token = token
        )),
        action_row(vec![user_select(
            &crate::custom_id::ui::developer::custom_id_for(
                crate::custom_id::ui::developer::Screen::Connect,
                client_id,
            ),
            message!("developer.application.019"),
        )]),
    ]);

    container(Some(WORKING), children)
}

/// Editing a set starts with its saved values checked, so adding a choice retains the others.
fn setting_option(value: &str, label: &str, selected: bool) -> Value {
    let mut option = select_option(value, label, None);
    option["default"] = Value::Bool(selected);
    option
}

/// One field that is free text: what it is now, and the button that opens its form.
///
/// The button is the section's accessory, which is what a section is for — the control sits
/// beside the text it acts on rather than in a row somewhere else.
fn editing(client_id: &str, field: &str, label: &str, now: Option<&str>) -> Value {
    section(
        vec![text(format!(
            "**{label}**（`{field}`）\n{}",
            now.unwrap_or(message!("developer.editing.001"))
        ))],
        button(
            &crate::custom_id::ui::developer::custom_id_for_field(
                crate::custom_id::ui::developer::Screen::Edit,
                client_id,
                field,
            ),
            message!("developer.editing.002"),
            ButtonStyle::Primary,
        ),
    )
}

/// Redirect URIs as text beside their edit button.
fn listing(values: &[String]) -> String {
    if values.is_empty() {
        message!("developer.listing.001").to_owned()
    } else {
        values.join(", ")
    }
}

/// The default subscription: everything, spelled out — so an application that
/// never names one gets every event, and no screen has to pretend an empty set
/// is full.
pub fn all_events() -> &'static [i64] {
    EVENT_TYPES
}

/// The name a notification `type` value goes by in the menu.
fn event_name(kind: i64) -> &'static str {
    match kind {
        2 => message!("developer.event_name.001"),
        3 => message!("developer.event_name.002"),
        4 => message!("developer.event_name.003"),
        _ => message!("developer.event_name.004"),
    }
}

/// The secret as it appears on the application's screen.
///
fn secret_block(secret: Option<&str>) -> Value {
    match secret {
        Some(secret) => text(format!(
            message!("developer.secret_block.001"),
            secret = secret
        )),
        None => text(message!("developer.secret_block.002")),
    }
}

/// Add the outcome without replacing the saved settings or exceeding the component limit.
pub fn saved(mut screen: Value, notice: &str) -> Value {
    let heading = if screen["components"][0]["type"] == 9 {
        &mut screen["components"][0]["components"][0]["content"]
    } else {
        &mut screen["components"][0]["content"]
    };
    *heading = Value::String(format!(
        "**{notice}**\n\n{}",
        heading.as_str().unwrap_or_default()
    ));
    screen
}

fn return_to_application(client_id: &str, label: &str) -> Value {
    use crate::custom_id::ui::developer::{Screen, custom_id_for};
    button(
        &custom_id_for(Screen::Show, client_id),
        label,
        ButtonStyle::Secondary,
    )
}

pub fn rotate_secret_confirmation(client_id: &str) -> Value {
    use crate::custom_id::ui::developer::{Screen, custom_id_for_field};
    container(
        Some(WORKING),
        vec![
            text(format!(
                message!("developer.rotate_secret_confirmation.001"),
                client_id = client_id
            )),
            action_row(vec![
                button(
                    &custom_id_for_field(Screen::RotateSecret, client_id, "confirm"),
                    message!("developer.rotate_secret_confirmation.002"),
                    ButtonStyle::Danger,
                ),
                return_to_application(
                    client_id,
                    message!("developer.rotate_secret_confirmation.003"),
                ),
            ]),
        ],
    )
}

pub fn connect_confirmation(client_id: &str, current: Option<i64>, bot: i64) -> Value {
    use crate::custom_id::ui::developer::{Screen, custom_id_for_field};
    let current = current.map_or_else(
        || message!("developer.connect_confirmation.001").to_owned(),
        |id| format!("<@{id}>（ID: `{id}`）"),
    );
    container(
        Some(WORKING),
        vec![
            text(format!(
                message!("developer.connect_confirmation.002"),
                bot = bot,
                client_id = client_id,
                current = current
            )),
            action_row(vec![
                button(
                    &custom_id_for_field(Screen::ConfirmConnect, client_id, &bot.to_string()),
                    message!("developer.connect_confirmation.003"),
                    ButtonStyle::Primary,
                ),
                return_to_application(client_id, message!("developer.connect_confirmation.004")),
            ]),
        ],
    )
}

/// A form that was refused, in the service's own words.
///
/// `description` is `error_description` and it is printed as it came: it names the one thing
/// to fix, and a friendlier sentence would throw that away. It is absent when the failure is
/// this service's — the endpoint logs the reason and tells the caller nothing, and the
/// honest screen says the same.
pub fn refusal(action: &str, description: Option<&str>) -> Value {
    let why = match description {
        Some(description) => description.to_owned(),
        None => message!("developer.refusal.001").to_owned(),
    };

    container(
        Some(REFUSED),
        vec![
            text(format!(
                message!("developer.refusal.002"),
                action = action,
                why = why
            )),
            action_row(vec![button(
                &crate::custom_id::ui::developer::custom_id(
                    crate::custom_id::ui::developer::Screen::List,
                ),
                message!("developer.refusal.003"),
                ButtonStyle::Primary,
            )]),
        ],
    )
}

/// A short refusal without navigation, using the same red accent as other errors.
pub fn error(sentence: &str) -> Value {
    container(Some(REFUSED), vec![text(sentence)])
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
                text(message!("developer.connect_result.001")),
                action_row(vec![button(
                    &crate::custom_id::ui::developer::custom_id(
                        crate::custom_id::ui::developer::Screen::List,
                    ),
                    message!("developer.connect_result.002"),
                    ButtonStyle::Primary,
                )]),
            ],
        ),
        Some(description) => container(
            Some(REFUSED),
            vec![
                text(format!(
                    message!("developer.connect_result.003"),
                    description = description
                )),
                // No もう一度: the way to try again is the picker on the application's screen,
                // and a button that reopens a form is the thing this screen is not for. The
                // service's sentence is what the operator acts on, and it is above.
                action_row(vec![button(
                    &crate::custom_id::ui::developer::custom_id(
                        crate::custom_id::ui::developer::Screen::List,
                    ),
                    message!("developer.connect_result.004"),
                    ButtonStyle::Primary,
                )]),
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

        let screen = applications(&owned, &BTreeMap::new());
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
        let screen = applications(&[], &BTreeMap::new());

        // The command, not a button: a screen's buttons move or choose, and this screen has
        // nowhere to move to, so what it teaches is the command's name.
        assert!(first_text(&screen).contains("/application register"));
    }

    /// And the command it teaches is a link when Discord's ids are known, which is the rule
    /// every piece of this service's Discord prose follows.
    #[test]
    fn an_empty_list_links_the_command_it_teaches() {
        let ids = BTreeMap::from([("application register".to_owned(), 9)]);
        let screen = applications(&[], &ids);

        assert!(
            first_text(&screen).contains("</application register:9> で登録できます。"),
            "{screen}"
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
            applications(&[], &BTreeMap::new()),
            application(
                "id",
                None,
                None,
                None,
                Some("the-secret"),
                "https://example.test/applications/verification?q=id",
                Fields {
                    client_name: None,
                    redirect_uris: &[],
                    client_uri: None,
                    logo_uri: None,
                    webhook_url: None,
                    discord_support_server_invite_slug: None,
                    application_type: "web",
                    grant_types: &[],
                    response_types: &[],
                    subscribed_events: &[],
                },
            ),
            connect_result("id", Some("no")),
            connect_result("id", None),
        ] {
            walk(&screen, &mut found);
        }

        assert!(!found.is_empty());

        for id in found {
            // Not "looks like an id": every one has to decode back to one of the screens,
            // which is what the dispatcher will do with it. A readable string that happens
            // to look right would fail there, and a test that only checked the prefix
            // would not notice.
            let decoded = crate::custom_id::parse(&id);

            assert!(
                crate::custom_id::ui::developer::parse(&decoded).is_ok(),
                "{id} does not decode to a developer screen"
            );
        }
    }

    /// The screens are components and nothing else, which is what the flag promises; a
    /// stray `content` would be a 400 on the wire.
    #[test]
    fn no_screen_carries_content_or_embeds_at_the_top() {
        for screen in [
            applications(&[], &BTreeMap::new()),
            application(
                "id",
                None,
                Some(123),
                Some("https://example.test/logo.png"),
                None,
                "https://example.test/applications/verification?q=id",
                Fields {
                    client_name: None,
                    redirect_uris: &[],
                    client_uri: None,
                    logo_uri: None,
                    webhook_url: None,
                    discord_support_server_invite_slug: None,
                    application_type: "web",
                    grant_types: &[],
                    response_types: &[],
                    subscribed_events: &[],
                },
            ),
        ] {
            assert!(screen.get("content").is_none(), "{screen}");
            assert_eq!(screen["type"], 17);
        }
    }
}
