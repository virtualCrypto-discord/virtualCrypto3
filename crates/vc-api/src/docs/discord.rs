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
    ButtonStyle, action_row, button, link_button, section, select, select_option, separator, text,
    thumbnail,
};
use crate::state::Links;

/// Where the screens send somebody who wants to read more, once.
const FOOTER: &str = message!("docs.discord.text.001");

/// The list: what this bot is, every command with the sentence Discord shows for
/// it, the menu that opens one, and the way to the page a new person needs.
pub fn index(links: &Links, ids: &BTreeMap<String, u64>) -> Vec<Value> {
    let listing = super::showings()
        .iter()
        .map(|showing| listing_line(showing, ids))
        .collect::<Vec<_>>()
        .join("\n");

    vec![
        // The project's logo, as an accessory: the picture the Elixir deployment's site served
        // under the same path. It is one screen's accessory rather than every screen's, because
        // a picture beside every paragraph of an option list is noise — the list is where
        // somebody arrives, and where a mark belongs.
        section(
            vec![greeting("**VirtualCrypto**", INTRO)],
            thumbnail(&links.logo_url()),
        ),
        separator(),
        text(format!(
            message!("docs.discord.index.001"),
            listing = listing
        )),
        menu(),
        // 「はじめに」 was a line in the footer and nothing a person could press, and the page
        // is a screen of this service's rather than a jump to the site: the same document the
        // site renders, which is what makes it a button and not a link.
        action_row(vec![button(
            &crate::custom_id::ui::help::start(),
            message!("docs.discord.index.002"),
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
        Some(id) => mention(name, *id),
        None => format!("**/{name}**"),
    }
}

/// A command mention: Discord writes a command somebody can press as its name and its id
/// between angle brackets, and renders it as the name.
fn mention(name: &str, id: u64) -> String {
    format!("</{name}:{id}>")
}

/// The command a piece of prose opens with, the id it is filed under, and what follows it —
/// the space between them included.
///
/// Follow registered subcommands and groups before treating the remainder as
/// arguments. An unknown child is left unlinked, rather than linking its parent.
fn command_head<'a>(text: &'a str, ids: &BTreeMap<String, u64>) -> Option<(String, u64, &'a str)> {
    let rest = text.strip_prefix('/')?;
    let (first, mut after) = split_word(rest);
    let mut path = first.to_owned();
    let mut id = ids.get(&path).copied();
    while subcommands_of(&path, ids) {
        let Some(word) = after.strip_prefix(' ') else {
            break;
        };
        let (child, remaining) = split_word(word);
        path = format!("{path} {child}");
        id = ids.get(&path).copied();
        after = remaining;
    }
    Some((path, id?, after))
}

/// Whether a command has subcommands, which is what tells the word after it apart from the
/// first of its arguments.
fn subcommands_of(command: &str, ids: &BTreeMap<String, u64>) -> bool {
    let prefix = format!("{command} ");

    ids.keys().any(|key| key.starts_with(&prefix))
}

/// A word and what follows it.
fn split_word(text: &str) -> (&str, &str) {
    match text.find(' ') {
        Some(at) => (&text[..at], &text[at..]),
        None => (text, ""),
    }
}

/// Prose with the commands in it as mentions.
///
/// A command is written the only way a screen that cannot link one can write it — in a code
/// span, `` `/pay` `` — which is what the site renders and what Discord's own picker shows.
/// Discord *can* link one, so a span that opens with a registered command becomes that
/// command's mention: `</pay:123>`, and for an example that carries its arguments,
/// `</pay:123> \`unit:<枚数>\`` — the command is what is pressable, and what followed it keeps
/// the code it was written in.
///
/// A span naming nothing this application has is left exactly as it was written, code and all,
/// which is what a deployment that cannot read the ids gets for every span: the screens then
/// read as they always did rather than as something half-linked. So is a backtick without its
/// pair, and the rest of the text after it.
///
/// Public because the document is not the only Discord text that names a command: a handler's
/// own sentence — 「`/info` で確認できます」 — is read by the same person and reads the same way.
pub fn mentions(text: &str, ids: &BTreeMap<String, u64>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];

        // An unpaired backtick is literal text: nothing after it is a span, so the walk ends
        // here and the remainder is appended as it stands.
        let Some(close) = after.find('`') else {
            break;
        };

        out.push_str(&rest[..open]);
        out.push_str(&linked_span(&after[..close], ids));

        rest = &after[close + 1..];
    }

    out.push_str(rest);

    out
}

/// One code span, with the command it opens with as a mention.
fn linked_span(span: &str, ids: &BTreeMap<String, u64>) -> String {
    match command_head(span, ids) {
        None => format!("`{span}`"),
        Some((name, id, rest)) if rest.trim_start().is_empty() => mention(&name, id),
        Some((name, id, rest)) => format!("{} `{}`", mention(&name, id), rest.trim_start()),
    }
}

/// A page of the site, in Discord: its title, then its sections as the command screens render
/// theirs.
///
/// The same blocks the site shows, rendered from the same document — the point of one document
/// with three renderings is that this is not prose written twice.
pub fn page(page: &Page, links: &Links, ids: &BTreeMap<String, u64>) -> Vec<Value> {
    let mut children = vec![greeting(&format!("**{}**", page.title), page.summary)];

    for section in page.sections {
        children.push(text(section_text(section, links, ids)));
    }

    children.push(action_row(vec![
        button(
            &crate::custom_id::ui::help::index(),
            message!("docs.discord.page.001"),
            ButtonStyle::Secondary,
        ),
        link_button(
            &format!("{}/document/{}", links.site_url, page.slug),
            message!("docs.discord.page.002"),
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
            message!("docs.discord.command.001"),
            fence(showing.usage)
        )));
    }

    if !showing.options.is_empty() {
        children.push(text(options_text(&showing.options)));
    }

    for section in showing.sections {
        children.push(text(section_text(section, links, ids)));
    }

    children.push(menu());
    children.push(action_row(vec![
        button(
            &crate::custom_id::ui::help::index(),
            message!("docs.discord.command.002"),
            ButtonStyle::Secondary,
        ),
        link_button(
            &format!("{}/document/commands#{}", links.site_url, showing.name),
            message!("docs.discord.command.003"),
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
        message!("docs.discord.menu.001"),
        options,
    )])
}

/// One line of the list. The administrator bit is read off the registration
/// payload, so the four commands that ask for it say so without being told twice.
fn listing_line(showing: &Showing, ids: &BTreeMap<String, u64>) -> String {
    if showing.admin_only {
        format!(
            message!("docs.discord.listing_line.001"),
            named(&showing.name, ids),
            showing.description
        )
    } else {
        format!("- {} {}", named(&showing.name, ids), showing.description)
    }
}

/// The title and the sentence under it.
///
/// A Text Display rather than a section, because on its own it carries no accessory: the screen
/// with a picture on it is [`index`], which wraps this in the section that holds the logo. The
/// rest are the title, the sentence, and what the screen has to say.
fn greeting(title: &str, description: &str) -> Value {
    text(format!("{title}\n{description}"))
}

/// The options as a list, a subcommand's own options nested under it.
fn options_text(options: &[OptionLine]) -> String {
    let mut lines = vec![
        message!("docs.discord.options_text.001").to_owned(),
        String::new(),
    ];
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
            _ if option.required => message!("docs.discord.option_lines.001").to_owned(),
            _ => message!("docs.discord.option_lines.002").to_owned(),
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
fn section_text(section: &Section, links: &Links, ids: &BTreeMap<String, u64>) -> String {
    let body = section
        .blocks
        .iter()
        .map(|block| block_text(block, links, ids))
        .collect::<Vec<_>>()
        .join("\n\n");

    match section.heading {
        Some(heading) => format!("## {heading}\n\n{body}"),
        None => body,
    }
}

fn block_text(block: &Block, links: &Links, ids: &BTreeMap<String, u64>) -> String {
    match block {
        Block::Text(text) => mentions(&resolve(text, links), ids),
        // Discord reads a list out of the lines itself, so the marker is all
        // this has to write.
        Block::List(items) => items
            .iter()
            .map(|item| format!("- {}", mentions(&resolve(item, links), ids)))
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Code(lines) => fence(lines),
    }
}

/// A fence: the lines exactly as they were written, command and all.
///
/// Discord does not resolve a mention inside a code block — it shows the raw
/// `</pat:1551719234256637972>` rather than `/pat` — so a fence, which is what somebody types,
/// keeps the plain `/name` a mention would have swallowed. [`mentions`] is where a mention
/// belongs: prose is the only place Discord makes one pressable, and a fence is not prose.
fn fence(lines: &[&str]) -> String {
    format!("```\n{}\n```", lines.join("\n"))
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
    ///
    /// No ids, so no mention: everything is exactly as it was written, which is what a
    /// deployment that cannot read the application's commands answers with.
    #[test]
    fn the_blocks_become_markdown() {
        const BLOCKS: &[Block] = &[
            Block::Text("`/pay` で送ります。[サイト]({site})もどうぞ。"),
            Block::Code(&["/pay unit:v user:@x amount:1"]),
            Block::List(&["ひとつ", "ふたつ"]),
        ];

        assert_eq!(
            section_text(
                &Section {
                    heading: Some("例"),
                    blocks: BLOCKS,
                },
                &links(),
                &BTreeMap::new()
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

    fn links() -> crate::state::Links {
        crate::state::Links {
            site_url: "https://example.test".to_owned(),
            invite_url: "https://example.test/invite".to_owned(),
            support_guild_invite_url: "https://example.test/support".to_owned(),
        }
    }

    /// What Discord answers for this application's commands: two of them, and one
    /// subcommand — which is what a mention of a subcommand is written with.
    fn ids() -> BTreeMap<String, u64> {
        BTreeMap::from([
            ("pay".to_owned(), 1),
            ("claim".to_owned(), 2),
            ("claim make".to_owned(), 2),
        ])
    }

    /// A span that names a command is that command's mention, and one that names anything
    /// else is left as the code it was written as: an address, a command this application
    /// does not have, a subcommand it was not given an id for.
    #[test]
    fn a_command_in_prose_is_a_mention() {
        let ids = ids();

        assert_eq!(
            mentions("`/pay` で送ります。", &ids),
            "</pay:1> で送ります。"
        );
        assert_eq!(
            mentions("既にある通貨を `/info` で確認してください。", &ids),
            "既にある通貨を `/info` で確認してください。",
            "this application has no `/info` in the map"
        );
        assert_eq!(
            mentions(
                "`/api/v2/currencies` と `/document/commands` はそのまま。",
                &ids
            ),
            "`/api/v2/currencies` と `/document/commands` はそのまま。",
            "a span that is not a command is code"
        );
        assert_eq!(
            mentions("`/claim show` は1件を表示します。", &ids),
            "`/claim show` は1件を表示します。",
            "an unregistered subcommand cannot be named by a mention"
        );
        assert_eq!(
            mentions("真ん中に `/pay` がある文。", &ids),
            "真ん中に </pay:1> がある文。",
            "and it need not open the text"
        );
        assert_eq!(
            mentions("**`/pay` で作る**", &ids),
            "**</pay:1> で作る**",
            "the mark around it is left alone"
        );
    }

    /// The arguments of an example are what the person reads rather than what they press, so
    /// only the command becomes a link — and what follows it keeps its code span.
    #[test]
    fn an_example_keeps_its_arguments_as_code() {
        let ids = ids();

        assert_eq!(
            mentions(
                "`/pay unit:<枚数> user:<送信先>` のように入力します。",
                &ids
            ),
            "</pay:1> `unit:<枚数> user:<送信先>` のように入力します。"
        );
        assert_eq!(
            mentions("`/claim make` で請求を作ります。", &ids),
            "</claim make:2> で請求を作ります。",
            "a subcommand is a mention of its own"
        );
        assert_eq!(
            mentions("`/claim make user:<請求先>` と入力します。", &ids),
            "</claim make:2> `user:<請求先>` と入力します。",
            "the command is as long as its path, and no longer"
        );
    }

    /// A backtick with no pair is text, and so is everything after it: there is no span to
    /// read a command out of, and inventing one would swallow the sentence.
    #[test]
    fn an_unpaired_backtick_ends_the_walk() {
        assert_eq!(
            mentions("`/pay` のあとに ` ひとつ。", &ids()),
            "</pay:1> のあとに ` ひとつ。"
        );
    }

    /// A fence is what somebody types, so every line stays exactly as it was written — a mention
    /// goes in prose, because Discord does not resolve one inside a code block and would show
    /// the raw `</claim make:2>`. The ids are in hand and unused: a fence has no place for one.
    #[test]
    fn a_fence_keeps_the_command_as_it_was_written() {
        assert!(ids().contains_key("claim make"), "an id to put somewhere");

        assert_eq!(
            fence(&[
                "/claim make user:<請求先> unit:<単位> amount:<枚数>",
                "/claim show id:<請求番号>",
            ]),
            "```\n\
             /claim make user:<請求先> unit:<単位> amount:<枚数>\n\
             /claim show id:<請求番号>\n\
             ```",
            "an id for the subcommand is not enough to put a mention in a fence"
        );
        assert_eq!(fence(&["/bal"]), "```\n/bal\n```");
    }
}
