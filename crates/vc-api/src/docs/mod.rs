//! The user-facing documentation, written once and rendered twice.
//!
//! `/help` in Discord and the site's `/document/*` pages say the same things:
//! what this service is for, how each command is typed, what happens to the
//! money. Written by hand, either one is the other written again, and the two
//! drift the first time only one of them is edited — the old `/help` pointed at
//! `{site}/document/commands`, which had never existed here. So the prose lives
//! in this module and two renderers read it:
//!
//! - [`discord`] draws the screens `/help` answers with, and the screens the
//!   menu on them moves between;
//! - [`json`] draws the document `GET /api/docs` serves, which the SPA renders
//!   at `/document/*`.
//!
//! **What is not here is everything the service already says.** A command's
//! name, the sentence Discord's own picker shows for it, and its options with
//! their requiredness are read out of [`crate::discord_commands`], which is the
//! payload Discord is registered with. A description written here as well would
//! be a second place for it to be wrong, and the one Discord shows would win. So
//! the split is:
//!
//! - the shape is the registration payload's;
//! - the prose is this module's;
//! - [`showings`] joins them, and `tests/documentation.rs` holds the join to
//!   saying something about every registered command.
//!
//! # The inline marks, and the whole of them
//!
//! Prose may carry three marks: `**bold**`, `` `code` ``, and `[label](url)`.
//! Discord renders them as they are — a Text Display is markdown — and [`json`]
//! takes them apart into spans so the SPA has nothing to parse and nothing to
//! escape. Anything that is not one of the three is literal text, which is what
//! keeps `usage` lines like `/pay amount:<枚数>` readable in both.
//!
//! One of those three marks reads differently on each screen, and that is the
//! point of having both: a code span that names a command —— `` `/claim make` ``,
//! or `` `/pay unit:<枚数>` `` with the arguments somebody would type —— is a link
//! on Discord, because [`discord`] writes it as Discord's own command mention and
//! Discord makes that pressable. [`json`] renders the span as the code it was
//! written as, which is what the site can do with it: a page there is read, and a
//! command is typed somewhere else.
//!
//! # The placeholders
//!
//! Addresses are written as `{site}`, `{invite}` and `{support}` and filled in
//! by [`resolve`] from [`Links`], because the deployment's URLs are not
//! compile-time constants: a test deployment and the real one are the same
//! strings with different addresses in them.

pub mod api;
pub mod commands;
pub mod discord;
pub mod json;
pub mod pages;

use serde_json::Value;

use crate::discord_commands;
use crate::state::Links;

/// One paragraph, list or fence of a section.
pub enum Block {
    /// Prose, carrying the three marks above.
    Text(&'static str),
    /// Bullet points, in the order they are written.
    List(&'static [&'static str]),
    /// The lines of a fenced block, without the fence.
    Code(&'static [&'static str]),
}

/// A headed part of a page, or of one command's screen.
///
/// The heading is optional because a page may open with a paragraph before it
/// has anything to head.
pub struct Section {
    pub heading: Option<&'static str>,
    pub blocks: &'static [Block],
}

/// A command's prose: everything both renderings need that the registration
/// payload does not carry.
pub struct Command {
    /// The name the registration payload and the handler dispatch use. The only
    /// way this module and Discord can be pointing at different commands.
    pub name: &'static str,
    /// How the command is typed, one line per form. `< >` marks a value to be
    /// replaced, and the arguments themselves are listed from the payload.
    pub usage: &'static [&'static str],
    pub sections: &'static [Section],
}

/// What a page shows under its own prose.
///
/// A page is mostly prose, but the two that are lists are lists: the document
/// says which list belongs under a page rather than the renderers deciding by
/// slug, so a page cannot end up with the wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    None,
    Commands,
    Endpoints,
}

/// A page of the site that is not one command: はじめに, アプリ連携, and so on.
pub struct Page {
    pub slug: &'static str,
    pub title: &'static str,
    /// One line, for the navigation.
    pub summary: &'static str,
    pub listing: Listing,
    pub sections: &'static [Section],
}

/// One command as both renderings show it: what is registered, and what has been
/// written about it.
pub struct Showing {
    pub name: String,
    /// What Discord's picker says, and what both renderings open with.
    pub description: String,
    /// Whether the registration payload asks for the administrator bit
    /// (`default_member_permissions: "0"`). Read rather than written down: the
    /// four commands that carry it are the four the handlers check the bit for,
    /// and a fifth would be one decision in two places.
    pub admin_only: bool,
    pub usage: &'static [&'static str],
    pub options: Vec<OptionLine>,
    pub sections: &'static [Section],
}

/// One option as Discord registered it.
pub struct OptionLine {
    pub name: String,
    pub description: String,
    /// A subcommand is not required in Discord's sense of the field, so this is
    /// false for one and the renderers say 「サブコマンド」 instead.
    pub required: bool,
    pub kind: OptionKind,
    pub autocomplete: bool,
    /// A subcommand's own options. Empty for every other kind.
    pub children: Vec<OptionLine>,
}

/// What an option takes, which is Discord's type number read as a word.
///
/// The numbers are the ones the registration payload uses: 1 subcommand, 3
/// string, 4 integer, 5 boolean, 6 user. A type this service does not register
/// is an [`OptionKind::Text`], because that is what Discord's own default is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Subcommand,
    Text,
    Integer,
    Boolean,
    User,
}

impl OptionKind {
    fn from_discord(kind: i64) -> Self {
        match kind {
            1 => OptionKind::Subcommand,
            4 => OptionKind::Integer,
            5 => OptionKind::Boolean,
            6 => OptionKind::User,
            _ => OptionKind::Text,
        }
    }

    /// What the option is, as a person reads it.
    pub fn label(self) -> &'static str {
        match self {
            OptionKind::Subcommand => "サブコマンド",
            OptionKind::Text => "文字列",
            OptionKind::Integer => "整数",
            OptionKind::Boolean => "真偽値",
            OptionKind::User => "ユーザー",
        }
    }
}

/// One sentence above the command list on `/help`: what the bot is.
///
/// Here rather than in [`pages`] because it is the one thing on the screen that
/// is not a command, and the landing page says its own version of it in its own
/// words.
pub const INTRO: &str = "VirtualCryptoは、Discordのサーバーで独自の通貨を使えるようにするBotです。";

/// The site's own address, as a page writes it.
pub const SITE: &str = "{site}";
/// The bot's invite, as a page writes it.
pub const INVITE: &str = "{invite}";
/// The support guild's invite, as a page writes it.
pub const SUPPORT: &str = "{support}";

/// Every address a page may write. A test refuses a fourth, because an address
/// nothing fills in is a page with `{...}` in it.
pub const PLACEHOLDERS: [&str; 3] = [SITE, INVITE, SUPPORT];

/// Fill the addresses in.
pub fn resolve(text: &str, links: &Links) -> String {
    text.replace(SITE, &links.site_url)
        .replace(INVITE, &links.invite_url)
        .replace(SUPPORT, &links.support_guild_invite_url)
}

/// Every command, in the order Discord is told about them.
pub fn showings() -> Vec<Showing> {
    discord_commands::commands()
        .iter()
        .filter_map(showing)
        .collect()
}

/// One command, by the name Discord knows it by.
pub fn showing_of(name: &str) -> Option<Showing> {
    showings().into_iter().find(|showing| showing.name == name)
}

/// One page of the site, by its slug — what a Discord screen that shows a page reads.
pub fn page_of(slug: &str) -> Option<&'static Page> {
    pages::all().iter().find(|page| page.slug == slug)
}

/// A registered command, with the prose that was written for it.
///
/// `None` is a payload without a name, which the registration tests already
/// refuse; a payload without prose is not: the prose is found by name, and the
/// documentation tests are what insist every registered command has one.
fn showing(command: &Value) -> Option<Showing> {
    let name = command.get("name")?.as_str()?;
    let prose = commands::all().iter().find(|entry| entry.name == name);

    Some(Showing {
        name: name.to_owned(),
        description: text_of(command.get("description")),
        admin_only: command
            .get("default_member_permissions")
            .and_then(Value::as_str)
            == Some("0"),
        usage: prose.map(|entry| entry.usage).unwrap_or_default(),
        options: options_of(command.get("options")),
        sections: prose.map(|entry| entry.sections).unwrap_or_default(),
    })
}

fn options_of(options: Option<&Value>) -> Vec<OptionLine> {
    options
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(option_of).collect())
        .unwrap_or_default()
}

/// One option, and its own options when it is a subcommand.
fn option_of(option: &Value) -> Option<OptionLine> {
    let name = option.get("name")?.as_str()?;

    Some(OptionLine {
        name: name.to_owned(),
        description: text_of(option.get("description")),
        required: option.get("required").and_then(Value::as_bool) == Some(true),
        kind: OptionKind::from_discord(option.get("type").and_then(Value::as_i64).unwrap_or(3)),
        autocomplete: option.get("autocomplete").and_then(Value::as_bool) == Some(true),
        children: options_of(option.get("options")),
    })
}

fn text_of(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// `Block::Text`, as a constructor: the tables below are data, and a function per
/// shape reads better at every entry than the enum's path.
pub const fn text(value: &'static str) -> Block {
    Block::Text(value)
}

pub const fn list(items: &'static [&'static str]) -> Block {
    Block::List(items)
}

pub const fn code(lines: &'static [&'static str]) -> Block {
    Block::Code(lines)
}

pub const fn section(heading: &'static str, blocks: &'static [Block]) -> Section {
    Section {
        heading: Some(heading),
        blocks,
    }
}

/// A section of prose with nothing above it: the paragraph a page opens with, or
/// the sentence under a heading that already said it.
pub const fn lead(blocks: &'static [Block]) -> Section {
    Section {
        heading: None,
        blocks,
    }
}
