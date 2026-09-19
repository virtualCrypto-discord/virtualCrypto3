//! The screens `/help` answers with.
//!
//! Two screens, and the menu that moves between them: the list of commands, and
//! one command in full. Both are built from [`super::showings`], so what a person
//! reads here is what Discord's own picker offers and what the site's pages say —
//! the three are one text with three renderings.
//!
//! A screen is a container's children rather than a response: the caller wraps it
//! in the ephemeral, components-only message every command answers with, which is
//! `crate::command::help`'s job. That also lets `/application help` show the same
//! command screen with its own button beside it.

use serde_json::Value;

use super::{Block, INTRO, OptionKind, OptionLine, Section, Showing, resolve};
use crate::components::{
    ButtonStyle, action_row, button, link_button, section, select, select_option, separator, text,
    thumbnail,
};
use crate::state::Links;

/// Where the screens send somebody who wants to read more, once.
const FOOTER: &str = "[コマンドの使い方]({site}/document/commands) — [Botの招待]({invite}) — \
                      [サポートサーバー]({support})";

/// The list: what this bot is, every command with the sentence Discord shows for
/// it, and the menu that opens one.
pub fn index(links: &Links) -> Vec<Value> {
    let listing = super::showings()
        .iter()
        .map(listing_line)
        .collect::<Vec<_>>()
        .join("\n");

    vec![
        greeting("**VirtualCrypto**", INTRO, links),
        separator(),
        text(format!("## コマンド\n\n{listing}")),
        menu(),
        text(resolve(FOOTER, links)),
    ]
}

/// One command: how it is typed, what its options are, and what has been written
/// about it.
pub fn command(showing: &Showing, links: &Links) -> Vec<Value> {
    let mut children = vec![greeting(
        &format!("**/{}**", showing.name),
        &showing.description,
        links,
    )];

    if !showing.usage.is_empty() {
        children.push(text(format!(
            "## 使い方\n\n```\n{}\n```",
            showing.usage.join("\n")
        )));
    }

    if !showing.options.is_empty() {
        children.push(text(options_text(&showing.options)));
    }

    for section in showing.sections {
        children.push(text(section_text(section, links)));
    }

    children.push(menu());
    children.push(action_row(vec![
        button(
            &crate::custom_id::ui::help::index(),
            "一覧に戻る",
            ButtonStyle::Secondary,
        ),
        link_button(
            &format!("{}/document/commands#{}", links.site_url, showing.name),
            "サイトで見る",
        ),
    ]));

    children
}

/// The menu: every command, by name, with the sentence Discord shows for it.
///
/// The choice carries the name, which is what the handler looks the command up
/// by, and the label carries a slash so it reads as something to type.
fn menu() -> Value {
    let options = super::showings()
        .iter()
        .map(|showing| {
            select_option(
                &showing.name,
                &format!("/{}", showing.name),
                Some(&showing.description),
            )
        })
        .collect();

    action_row(vec![select(
        &crate::custom_id::ui::help::select(),
        "コマンドを選ぶ",
        options,
    )])
}

/// One line of the list. The administrator bit is read off the registration
/// payload, so the four commands that ask for it say so without being told twice.
fn listing_line(showing: &Showing) -> String {
    if showing.admin_only {
        format!("- **/{}**（管理者） {}", showing.name, showing.description)
    } else {
        format!("- **/{}** {}", showing.name, showing.description)
    }
}

/// The title and the sentence under it, with the logo beside them.
///
/// A section is text with something next to it, and the logo is the only
/// accessory these screens have — the shape `/help` and `/invite` already had.
fn greeting(title: &str, description: &str, links: &Links) -> Value {
    section(
        vec![text(title), text(description)],
        thumbnail(&links.logo_url()),
    )
}

/// The options as a list, a subcommand's own options nested under it.
fn options_text(options: &[OptionLine]) -> String {
    let mut lines = vec!["## 引数".to_owned(), String::new()];
    lines.extend(option_lines(options, 0));

    if marked(options, |option| option.autocomplete) {
        lines.push(String::new());
        lines.push("候補から選べる引数は、入力しながら候補が表示されます。".to_owned());
    }

    lines.join("\n")
}

fn option_lines(options: &[OptionLine], depth: usize) -> Vec<String> {
    let mut lines = Vec::new();

    for option in options {
        // A subcommand is not required in Discord's sense of the field, so
        // saying 「任意」 of one would be saying something untrue.
        let mark = match option.kind {
            OptionKind::Subcommand => String::new(),
            _ if option.required => "（必須）".to_owned(),
            _ => "（任意）".to_owned(),
        };

        lines.push(format!(
            "{}- `{}`{} {}",
            "  ".repeat(depth),
            option.name,
            mark,
            option.description,
        ));

        lines.extend(option_lines(&option.children, depth + 1));
    }

    lines
}

fn marked(options: &[OptionLine], predicate: impl Fn(&OptionLine) -> bool + Copy) -> bool {
    options
        .iter()
        .any(|option| predicate(option) || marked(&option.children, predicate))
}

/// A section as one Text Display: a heading, then its blocks with a blank line
/// between them — which is how markdown says "a new paragraph".
fn section_text(section: &Section, links: &Links) -> String {
    let body = section
        .blocks
        .iter()
        .map(|block| block_text(block, links))
        .collect::<Vec<_>>()
        .join("\n\n");

    match section.heading {
        Some(heading) => format!("## {heading}\n\n{body}"),
        None => body,
    }
}

fn block_text(block: &Block, links: &Links) -> String {
    match block {
        Block::Text(text) => resolve(text, links),
        // Discord reads a list out of the lines itself, so the marker is all
        // this has to write.
        Block::List(items) => items
            .iter()
            .map(|item| format!("- {}", resolve(item, links)))
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Code(lines) => format!("```\n{}\n```", lines.join("\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command that takes a name, one that takes nothing, and a subcommand
    /// with an option of its own — the three shapes the list has to read well.
    fn options() -> Vec<OptionLine> {
        vec![
            OptionLine {
                name: "unit".to_owned(),
                description: "送信したい通貨の単位です。".to_owned(),
                required: true,
                kind: OptionKind::Text,
                autocomplete: true,
                children: Vec::new(),
            },
            OptionLine {
                name: "amount".to_owned(),
                description: "送信する通貨の量です。".to_owned(),
                required: true,
                kind: OptionKind::Integer,
                autocomplete: false,
                children: Vec::new(),
            },
            OptionLine {
                name: "list".to_owned(),
                description: "一覧を表示します。".to_owned(),
                required: false,
                kind: OptionKind::Subcommand,
                autocomplete: false,
                children: vec![OptionLine {
                    name: "pending".to_owned(),
                    description: "未処理の請求を表示します。".to_owned(),
                    required: false,
                    kind: OptionKind::Boolean,
                    autocomplete: false,
                    children: Vec::new(),
                }],
            },
        ]
    }

    /// Requiredness is said, subcommands are not given one, and a subcommand's
    /// own options are under it.
    #[test]
    fn the_options_read_as_a_list() {
        assert_eq!(
            options_text(&options()),
            "## 引数\n\
             \n\
             - `unit`（必須） 送信したい通貨の単位です。\n\
             - `amount`（必須） 送信する通貨の量です。\n\
             - `list` 一覧を表示します。\n\
             \x20\x20- `pending`（任意） 未処理の請求を表示します。\n\
             \n\
             候補から選べる引数は、入力しながら候補が表示されます。"
        );
    }

    /// A command whose options are all typed says nothing about autocomplete.
    #[test]
    fn the_autocomplete_note_appears_only_when_something_offers_one() {
        let typed = vec![OptionLine {
            name: "amount".to_owned(),
            description: "量です。".to_owned(),
            required: true,
            kind: OptionKind::Integer,
            autocomplete: false,
            children: Vec::new(),
        }];

        assert!(!options_text(&typed).contains("候補"));
    }

    /// The three blocks, as Discord reads them: a fence keeps its newlines, a
    /// list keeps its markers, and the addresses are the deployment's.
    #[test]
    fn the_blocks_become_markdown() {
        const BLOCKS: &[Block] = &[
            Block::Text("`/pay` で送ります。[サイト]({site})もどうぞ。"),
            Block::Code(&["/pay unit:v user:@x amount:1"]),
            Block::List(&["ひとつ", "ふたつ"]),
        ];

        let links = crate::state::Links {
            site_url: "https://example.test".to_owned(),
            invite_url: "https://example.test/invite".to_owned(),
            support_guild_invite_url: "https://example.test/support".to_owned(),
        };

        assert_eq!(
            section_text(
                &Section {
                    heading: Some("例"),
                    blocks: BLOCKS,
                },
                &links
            ),
            "## 例\n\
             \n\
             `/pay` で送ります。[サイト](https://example.test)もどうぞ。\n\
             \n\
             ```\n/pay unit:v user:@x amount:1\n```\n\
             \n\
             - ひとつ\n- ふたつ"
        );
    }
}
