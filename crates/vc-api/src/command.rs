//! The slash-command handlers and the responses they render.
//!
//! `VirtualCryptoWeb.Interaction.Command.handle/4` returns a term that
//! `VirtualCryptoWeb.Api.InteractionsJSON` renders, so the two are kept together
//! here.

use serde_json::{Value, json};

use crate::state::AppState;

/// `Interactions.Util`: response types and colours.
pub const PONG: i64 = 1;
pub const CHANNEL_MESSAGE_WITH_SOURCE: i64 = 4;
pub const UPDATE_MESSAGE: i64 = 7;
pub const EPHEMERAL: i64 = 64;
pub const COLOR_OK: i64 = 0x38EA42;
pub const COLOR_ERROR: i64 = 0xEA3875;
pub const COLOR_BRAND: i64 = 0x6221ED;

pub const ACTION_ROW: i64 = 1;
pub const BUTTON: i64 = 2;
pub const SELECT_MENU: i64 = 3;
pub const TEXT_INPUT: i64 = 4;
pub const BUTTON_STYLE_PRIMARY: i64 = 1;
pub const BUTTON_STYLE_SECONDARY: i64 = 2;
pub const BUTTON_STYLE_SUCCESS: i64 = 3;
pub const BUTTON_STYLE_DANGER: i64 = 4;
pub const BUTTON_STYLE_LINK: i64 = 5;
pub const TEXT_INPUT_STYLE_SHORT: i64 = 1;
pub const TEXT_INPUT_STYLE_PARAGRAPH: i64 = 2;

/// `Command.handle/4` for `help`, rendered by `InteractionsJSON.help/1`.
pub fn help(state: &AppState) -> Value {
    let links = state.links();

    let description = format!(
        "VirtualCryptoはDiscord上でサーバーに独自の通貨を作成できるBotです。\n\
         [コマンドの使い方の詳細]({site}/document/commands)\n\
         [公式サイト]({site})\n\
         [Botの招待]({bot})\n\
         [サポートサーバーの招待]({support})",
        site = links.site_url,
        bot = links.invite_url,
        support = links.support_guild_invite_url,
    );

    embed(description, links.logo_url())
}

/// `Command.handle/4` for `invite`, rendered by `InteractionsJSON.invite/1`.
pub fn invite(state: &AppState) -> Value {
    let links = state.links();

    let description = format!(
        "[Botの招待]({bot})\n[サポートサーバーの招待]({support})",
        bot = links.invite_url,
        support = links.support_guild_invite_url,
    );

    embed(description, links.logo_url())
}

/// The ephemeral, brand-coloured embed both commands share.
fn embed(description: String, logo_url: String) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": {
            "flags": EPHEMERAL,
            "embeds": [{
                "color": COLOR_BRAND,
                "title": "VirtualCrypto",
                "thumbnail": { "url": logo_url },
                "description": description,
            }],
        },
    })
}
