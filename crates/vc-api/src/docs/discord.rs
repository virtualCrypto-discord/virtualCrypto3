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

use std::collections::BTreeMap;

use serde_json::Value;

use super::{Block, INTRO, OptionKind, OptionLine, Page, Section, Showing, resolve};
use crate::components::{
    ButtonStyle, action_row, button, link_button, select, select_option, separator, text,
};
use crate::state::Links;

/// Where the screens send somebody who wants to read more, once.
const FOOTER: &str = "[コマンドの使い方]({site}/document/commands) — [Botの招待]({invite}) — \
                      [サポートサーバー]({support})";

/// The list: what this bot is, every command with the sentence Discord shows for
/// it, the menu that opens one, and the way to the page a new person needs.
pub fn index(links: &Links, ids: &BTreeMap<String, u64>) -> Vec<Value> {
    let listing = super::showings()
        .iter()
        .map(|showing| listing_line(showing, ids))
        .collect::<Vec<_>>()
        .join("\n");

    vec![
        greeting("**VirtualCrypto**", INTRO),
        separator(),
        text(format!("## コマンド\n\n{listing}")),
        menu(),
        // 「はじめに」 was a line in the footer and nothing a person could press, and the page
        // is a screen of this service's rather than a jump to the site: the same document the
        // site renders, which is what makes it a button and not a link.
        action_row(vec![button(
            &crate::custom_id::ui::help::start(),
            "はじめに",
            ButtonStyle::Primary,
        )]),
        text(resolve(FOOTER, links)),
    ]
}

/// A command's name as Discord reads it: a mention —— `</name:id>` —— when the application's
/// ids are known, which is a link somebody can press, and the name in bold when they are not.
///
/// The ids come from Discord and are read once per process; a deployment that cannot read
/// them says 「/name」 exactly as it always did.
fn named(name: &str, ids: &BTreeMap<String, u64>) -> String {
    match ids.get(name) {
        Some(id) => format!("</{name}:{id}>"),
        None => format!("**/{name}**"),
    }
}

/// A page of the site, in Discord: its title, then its sections as the command screens render
/// theirs.
///
/// The same blocks the site shows, rendered from the same document — the point of one document
/// with three renderings is that this is not prose written twice.
pub fn page(page: &Page, links: &Links) -> Vec<Value> {
    let mut children = vec![greeting(&format!("**{}**", page.title), page.summary)];

    for section in page.sections {
        children.push(text(section_text(section, links)));
    }

    children.push(action_row(vec![
        button(
            &crate::custom_id::ui::help::index(),
            "一覧に戻る",
            ButtonStyle::Secondary,
        ),
        link_button(
            &format!("{}/document/{}", links.site_url, page.slug),
            "サイトで見る",
        ),
    ]));

    children
}

/// One command: how it is typed, what its options are, and what has been written
/// about it.
pub fn command(showing: &Showing, links: &Links, ids: &BTreeMap<String, u64>) -> Vec<Value> {
    let mut children = vec![greeting(&named(&showing.name, ids), &showing.description)];

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
fn listing_line(showing: &Showing, ids: &BTreeMap<String, u64>) -> String {
    if showing.admin_only {
        format!(
            "- {}（管理者） {}",
            named(&showing.name, ids),
            showing.description
        )
    } else {
        format!("- {} {}", named(&showing.name, ids), showing.description)
    }
}

/// The title and the sentence under it.
///
/// Both screens carried a thumbnail of the site's logo, which this deployment does not serve:
/// `/static/images/logo.jpg` was the old site's path and the SPA has no `static/` at all, so
/// what a person saw was a broken image. A screen that cannot show a picture is a screen with
/// no accessory, which a section requires — so the title and the sentence are a Text Display.
fn greeting(title: &str, description: &str) -> Value {
    text(format!("{title}\n{description}"))
}

/// The options as a list, a subcommand's own options nested under it.
fn options_text(options: &[OptionLine]) -> String {
    let mut lines = vec!["## 引数".to_owned(), String::new()];
    lines.extend(option_lines(options, 0));

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
             \x20\x20- `pending`（任意） 未処理の請求を表示します。"
        );
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
