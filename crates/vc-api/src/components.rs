//! Discord's message components, as far as this service sends them.
//!
//! Read from `docs.discord.com/developers/components/reference` and the Message Flags
//! table, and written in one place so a screen is data rather than a `json!` inside a
//! handler.
//!
//! Only the components this service actually uses are here — Container, Text Display,
//! Separator, Action Row, Button, Section, Thumbnail, String Select — because a wrapper
//! for a component nothing sends is a wrapper nobody has checked.
//!
//! Two things this module exists to keep in one place:
//!
//! - **The two flags.** `EPHEMERAL` so only the invoker sees the answer, and
//!   `IS_COMPONENTS_V2` so the message is components rather than content. Both values are
//!   from Discord's Message Flags table; `1 << 6` and `1 << 15`.
//! - **The reset the components flag requires.** Under `IS_COMPONENTS_V2` a message may
//!   not carry `content`, `embeds`, `sticker_ids` or `poll` — on a create that is a 400,
//!   and on an edit they must be *set to empty* rather than omitted, which is also a 400.
//!   Every screen after the first is an edit of the first, so [`ephemeral`] writes them
//!   empty every time and no handler has to remember.

use serde_json::{Value, json};

/// Only the user who invoked the interaction sees the response.
pub const EPHEMERAL: u64 = 1 << 6;

/// The interaction callback that opens a modal, which is a response and not a message.
pub const MODAL: i64 = 9;

/// The message is components and nothing else. Once set on a message, it cannot be
/// removed.
pub const IS_COMPONENTS_V2: u64 = 1 << 15;

/// What an ephemeral, components-only interaction response looks like.
///
/// Used for every screen: `content` and `embeds` empty as the flag requires, `components`
/// as given. What the handler does with the result — answer with it, or edit the message
/// it is already on — is the handler's business.
pub fn ephemeral(components: Vec<Value>) -> Value {
    json!({
        "flags": EPHEMERAL | IS_COMPONENTS_V2,
        "content": Value::Null,
        "embeds": [],
        "components": components,
    })
}

/// A button's style, which is also what decides which fields it may carry: a Link button
/// has a `url` and no `custom_id`, and every other style has a `custom_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    /// The most important action in a group. One per row.
    Primary = 1,
    /// Supporting actions, and what to use for several of equal weight.
    Secondary = 2,
    Success = 3,
    /// An action with irreversible consequences.
    Danger = 4,
}

/// A button that sends an interaction back when pressed.
pub fn button(custom_id: &str, label: &str, style: ButtonStyle) -> Value {
    json!({
        "type": 2,
        "style": style as u8,
        "custom_id": custom_id,
        "label": label,
    })
}

/// A button that opens a URL and, because it does, sends nothing back.
pub fn link_button(url: &str, label: &str) -> Value {
    json!({
        "type": 2,
        "style": 5,
        "url": url,
        "label": label,
    })
}

/// A menu that may take more than one of its options.
///
/// A string select is the one select that works in a message *and* is given its options by the
/// service, which is what makes it the thing to edit a set with — `grant_types` is two values
/// out of two, and the set that is chosen is the set that is sent.
pub fn select_many(
    custom_id: &str,
    placeholder: &str,
    options: Vec<Value>,
    min_values: u8,
    max_values: u8,
) -> Value {
    json!({
        "type": 3,
        "custom_id": custom_id,
        "placeholder": placeholder,
        "options": options,
        "min_values": min_values,
        "max_values": max_values,
    })
}

/// A picker for a user, which is how a bot is chosen.
///
/// A bot is a user, so Discord already has the thing to ask with, and asking somebody to paste
/// a snowflake is what this screen exists to avoid. Only a message may carry one — a modal
/// takes text inputs and nothing else — which is why connecting offers a picker here instead of
/// opening a form.
pub fn user_select(custom_id: &str, placeholder: &str) -> Value {
    json!({
        "type": 5,
        "custom_id": custom_id,
        "placeholder": placeholder,
        "min_values": 1,
        "max_values": 1,
    })
}

/// Up to five buttons, or one select, or a single select menu.
pub fn action_row(children: Vec<Value>) -> Value {
    json!({ "type": 1, "components": children })
}

/// Markdown text. Under the components flag this is what replaces a message's `content`.
pub fn text(content: impl Into<String>) -> Value {
    json!({ "type": 10, "content": content.into() })
}

pub fn separator() -> Value {
    json!({ "type": 14 })
}

pub fn thumbnail(url: &str) -> Value {
    json!({ "type": 11, "media": { "url": url } })
}

/// Text with something beside it. The accessory is a [`button`] or a [`thumbnail`].
pub fn section(children: Vec<Value>, accessory: Value) -> Value {
    json!({ "type": 9, "components": children, "accessory": accessory })
}

/// A group of components with an optional accent. This is the top level of every screen.
pub fn container(accent_color: Option<u32>, children: Vec<Value>) -> Value {
    let mut value = json!({ "type": 17, "components": children });

    if let Some(color) = accent_color {
        value["accent_color"] = json!(color);
    }

    value
}

/// The response that opens a modal.
///
/// A modal is a callback rather than a message, so this is not something to put in
/// [`ephemeral`]: what opens is a form, and what it submits comes back as its own
/// interaction.
pub fn modal(custom_id: &str, title: &str, components: Vec<Value>) -> Value {
    json!({
        "type": MODAL,
        "data": {
            "custom_id": custom_id,
            "title": title,
            "components": components,
        },
    })
}

/// How tall a text input is. Nothing else about it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputStyle {
    /// One line: a name, an id, a url.
    Short = 1,
    /// Many: a list of redirect uris, one per line.
    Paragraph = 2,
}

/// A labelled field.
///
/// The label is what a person reads, so it is required and capped at 45 characters, with
/// an optional description under it at 100. A text input inside an Action Row is the
/// deprecated form; Discord asks for this one.
pub fn label(name: &str, description: Option<&str>, component: Value) -> Value {
    let mut value = json!({
        "type": 18,
        "label": truncate(name, 45),
        "component": component,
    });

    if let Some(description) = description {
        value["description"] = json!(truncate(description, 100));
    }

    value
}

/// A field's input.
///
/// `value` pre-fills it, which is what makes an edit a correction rather than a retyping:
/// the fields `PATCH /oauth2/clients/@me` leaves alone are the ones sent back unchanged.
pub fn text_input(
    custom_id: &str,
    style: TextInputStyle,
    required: bool,
    max_length: Option<u64>,
    value: Option<&str>,
) -> Value {
    let mut input = json!({
        "type": 4,
        "custom_id": custom_id,
        "style": style as u8,
        "required": required,
    });

    if let Some(max_length) = max_length {
        input["max_length"] = json!(max_length);
    }

    if let Some(value) = value {
        input["value"] = json!(truncate(value, 4000));
    }

    input
}

/// One application in a [`select`].
///
/// Discord caps an option's label and value at 100 characters. A `client_id` is a uuid and
/// fits; a `client_name` is whatever somebody typed, so it is trimmed and the uuid stays
/// the value — a label the server answers with is not one to trust either.
pub fn select_option(value: &str, label: &str, description: Option<&str>) -> Value {
    let mut option = json!({
        "label": truncate(label, 100),
        "value": truncate(value, 100),
    });

    if let Some(description) = description {
        option["description"] = json!(truncate(description, 100));
    }

    option
}

/// A menu of the caller's applications.
pub fn select(custom_id: &str, placeholder: &str, options: Vec<Value>) -> Value {
    json!({
        "type": 3,
        "custom_id": custom_id,
        "placeholder": truncate(placeholder, 150),
        "options": options,
    })
}

/// Discord counts in characters, not bytes, and a `client_name` may be Japanese.
fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two numbers, from Discord's Message Flags table. Asserted because they are the
    /// whole reason this module exists and a bit shift is easy to get wrong quietly.
    #[test]
    fn the_flags_are_the_ones_discord_documents() {
        assert_eq!(EPHEMERAL, 64);
        assert_eq!(IS_COMPONENTS_V2, 32768);
        assert_eq!(EPHEMERAL | IS_COMPONENTS_V2, 32832);
    }

    /// The reset the components flag requires, on every response rather than on the edits
    /// that need it: no handler should have to know which of these is a create.
    #[test]
    fn every_response_resets_the_fields_the_flag_forbids() {
        let response = ephemeral(vec![text("hello")]);

        assert_eq!(response["flags"], json!(32832));
        assert_eq!(response["content"], Value::Null);
        assert_eq!(response["embeds"], json!([]));
        assert_eq!(response["components"][0]["type"], json!(10));
    }

    #[test]
    fn a_button_says_which_style_it_is() {
        assert_eq!(
            button("dev:home", "戻る", ButtonStyle::Secondary)["style"],
            2
        );
        assert_eq!(button("dev:home", "登録", ButtonStyle::Primary)["style"], 1);
        assert_eq!(button("x", "消す", ButtonStyle::Danger)["style"], 4);

        // A link button carries a url and no custom_id, because it sends nothing back.
        let link = link_button("https://example.test/", "読む");

        assert_eq!(link["style"], 5);
        assert!(link.get("custom_id").is_none(), "{link}");
    }

    /// A container without an accent has no `accent_color` key at all, rather than a null
    /// one: Discord reads an absent optional and a null differently elsewhere, and there
    /// is no reason to find out whether it does here.
    #[test]
    fn a_container_without_an_accent_omits_the_field() {
        assert!(container(None, vec![]).get("accent_color").is_none());
        assert_eq!(container(Some(0x00FF00), vec![])["accent_color"], 0x00FF00);
    }

    /// Truncation is by characters, because a Japanese `client_name` is three bytes a
    /// character and Discord counts the characters.
    #[test]
    fn a_long_label_is_cut_by_characters() {
        let name = "あ".repeat(200);
        let option = select_option("3f1c2f4e-9a11-4d2b-8c3e-5f6a7b8c9d0e", &name, None);

        let label = option["label"].as_str().expect("a label");

        assert_eq!(label.chars().count(), 100);
        assert_eq!(option["value"], "3f1c2f4e-9a11-4d2b-8c3e-5f6a7b8c9d0e");
    }
}

#[cfg(test)]
mod modal_tests {
    use super::*;

    /// A modal is a callback, not a message: no flags, and `data` carries the form.
    #[test]
    fn a_modal_is_the_callback_that_opens_it() {
        let opened = modal(
            "dev:register",
            "アプリケーションの登録",
            vec![label(
                "クライアント名",
                None,
                text_input("client_name", TextInputStyle::Short, true, None, None),
            )],
        );

        assert_eq!(opened["type"], json!(9));
        assert_eq!(opened["data"]["custom_id"], "dev:register");
        assert!(opened.get("flags").is_none(), "{opened}");
        assert_eq!(opened["data"]["components"][0]["type"], json!(18));
        assert_eq!(
            opened["data"]["components"][0]["component"]["type"],
            json!(4)
        );
    }

    /// Discord caps a label at 45 characters and a description at 100, and counts both in
    /// characters, so a Japanese label is three bytes each and still fits.
    #[test]
    fn a_label_is_cut_to_what_discord_accepts() {
        let long = "あ".repeat(100);
        let field = label(
            &long,
            Some(&long),
            text_input("x", TextInputStyle::Short, true, None, None),
        );

        assert_eq!(field["label"].as_str().expect("label").chars().count(), 45);
        assert_eq!(
            field["description"]
                .as_str()
                .expect("description")
                .chars()
                .count(),
            100
        );
    }

    /// A pre-filled input is what an edit sends back, and it is capped where Discord caps
    /// it rather than where a database column happens to end.
    #[test]
    fn an_input_prefills_and_says_which_style_it_is() {
        let input = text_input(
            "redirect_uris",
            TextInputStyle::Paragraph,
            true,
            Some(4000),
            Some("https://example.test/callback"),
        );

        assert_eq!(input["style"], 2);
        assert_eq!(input["max_length"], 4000);
        assert_eq!(input["value"], "https://example.test/callback");
        assert_eq!(input["required"], true);

        let short = text_input("client_name", TextInputStyle::Short, false, None, None);

        assert_eq!(short["style"], 1);
        assert!(short.get("value").is_none(), "{short}");
        assert!(short.get("max_length").is_none(), "{short}");
    }
}
