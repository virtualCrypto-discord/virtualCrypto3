//! `VirtualCryptoWeb.Interaction.CustomId`.
//!
//! Discord limits a `custom_id` to 100 characters, so the payload is packed: the
//! byte stream is read as 20-bit groups, each is offset by 65536 to land in the
//! supplementary plane, and the result is a string Discord will accept. The
//! first byte is a discriminator that `parse/1` drops.
//!
//! The final group is left-aligned, so [`parse`] always emits five bytes per pair
//! of groups and therefore leaves zero padding at the end. Elixir behaves the
//! same way, and the padding is harmless because real payloads append their data
//! after the two header bytes.

use thiserror::Error;

const OFFSET: u32 = 65536;

pub fn encode(prefix: u8, bytes: &[u8]) -> String {
    let mut input = Vec::with_capacity(bytes.len() + 1);
    input.push(prefix);
    input.extend_from_slice(bytes);

    let total = input.len() * 8;
    let mut groups = Vec::with_capacity(total.div_ceil(20));
    let mut bit = 0;

    while bit < total {
        let take = (total - bit).min(20);
        // The trailing bits of a short group are zero, which is the left-aligned
        // form Elixir's binary patterns produce.
        let group = read_bits(&input, bit, take) << (20 - take);
        groups.push(char::from_u32(group + OFFSET).unwrap_or(char::REPLACEMENT_CHARACTER));
        bit += take;
    }

    groups.into_iter().collect()
}

pub fn parse(encoded: &str) -> Vec<u8> {
    let groups: Vec<u32> = encoded
        .chars()
        .map(|character| (character as u32).saturating_sub(OFFSET))
        .collect();

    let mut out = Vec::with_capacity(groups.len().div_ceil(2) * 5);

    for chunk in groups.chunks(2) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);

        out.push(((a & 0x0FF000) >> 12) as u8);
        out.push(((a & 0x000FF0) >> 4) as u8);
        out.push((((a & 0x00000F) << 4) | ((b & 0x0F0000) >> 16)) as u8);
        out.push(((b & 0x00FF00) >> 8) as u8);
        out.push((b & 0x0000FF) as u8);
    }

    // Drop the discriminator byte.
    if out.is_empty() {
        out
    } else {
        out.split_off(1)
    }
}

fn read_bits(input: &[u8], start: usize, count: usize) -> u32 {
    let mut value = 0;

    for offset in 0..count {
        let position = start + offset;
        let bit = (input[position / 8] >> (7 - (position % 8))) & 1;
        value = (value << 1) | u32::from(bit);
    }

    value
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UiError {
    /// The discriminator nibble was not `0xF`, or the id was truncated.
    #[error("unrecognised custom_id")]
    Head,
    /// A valid discriminator with an id no handler knows. Elixir's `parse_long/1`
    /// has no fallback clause and raises here.
    #[error("unknown custom_id id {0}")]
    Unknown(u16),
}

/// The component payloads, mirroring `CustomId.UI.*`.
pub mod ui {
    use super::UiError;

    /// The head byte doubles as the high byte of the id: Elixir matches
    /// `<<h1, h2, data::binary>>` against the whole decoded payload.
    fn parse_id(source: &[u8]) -> Result<(u16, Vec<u8>), UiError> {
        let Some(&head) = source.first() else {
            return Err(UiError::Head);
        };

        if head >> 4 != 0x0F {
            return Err(UiError::Head);
        }

        let Some(&low) = source.get(1) else {
            return Err(UiError::Head);
        };

        Ok((
            ((head & 0x0F) as u16) << 8 | u16::from(low),
            source[2..].to_vec(),
        ))
    }

    pub mod button {
        use super::{UiError, parse_id};

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ListScope {
            All,
            Received,
            Claimed,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Action {
            Approve,
            Deny,
            Cancel,
            Back,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Path {
            List(ListScope),
            Act(Action),
            ActionSingle(Action),
        }

        pub fn claim_list(scope: ListScope) -> [u8; 2] {
            let id = match scope {
                ListScope::All => 1,
                ListScope::Received => 2,
                ListScope::Claimed => 3,
            };

            [0xF0, id]
        }

        pub fn claim_action(action: Action) -> [u8; 2] {
            let id = match action {
                Action::Approve => 4,
                Action::Deny => 5,
                Action::Cancel => 6,
                Action::Back => 7,
            };

            [0xF0, id]
        }

        pub fn claim_action_single(action: Action) -> [u8; 2] {
            let id = match action {
                Action::Approve => 8,
                Action::Deny => 9,
                Action::Cancel => 10,
                Action::Back => 11,
            };

            [0xF0, id]
        }

        pub fn parse(source: &[u8]) -> Result<(Path, Vec<u8>), UiError> {
            let (id, data) = parse_id(source)?;

            let path = match id {
                1 => Path::List(ListScope::All),
                2 => Path::List(ListScope::Received),
                3 => Path::List(ListScope::Claimed),
                4 => Path::Act(Action::Approve),
                5 => Path::Act(Action::Deny),
                6 => Path::Act(Action::Cancel),
                7 => Path::Act(Action::Back),
                8 => Path::ActionSingle(Action::Approve),
                9 => Path::ActionSingle(Action::Deny),
                10 => Path::ActionSingle(Action::Cancel),
                _ => return Err(UiError::Unknown(id)),
            };

            Ok((path, data))
        }
    }

    pub mod select_menu {
        use super::{UiError, parse_id};

        pub fn claim_select() -> [u8; 2] {
            [0xF0, 1]
        }

        pub fn parse(source: &[u8]) -> Result<([&'static str; 2], Vec<u8>), UiError> {
            let (id, data) = parse_id(source)?;

            match id {
                1 => Ok((["claim", "select"], data)),
                _ => Err(UiError::Unknown(id)),
            }
        }
    }

    /// The screens the DM's developer features move between.
    ///
    /// Not from the Elixir: it had no Discord surface for registering an application,
    /// listing your own or connecting a bot, so there is nothing to mirror. What is
    /// mirrored is the shape — a head byte, a numeric screen, and the payload after it —
    /// because Discord sends back only the `custom_id` and an ephemeral message cannot be
    /// looked up by id, so the screen and its subject both have to travel in this string.
    pub mod developer {
        use super::{UiError, parse_id};

        /// Which screen the component belongs to. The numbers are this module's, and
        /// they must not be renumbered: a message already in a DM carries them.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Screen {
            Home,
            List,
            Connect,
            /// The form that creates one. Not `Home`, which is the screen that offers it.
            Register,
            /// The form that changes one, as opposed to `Connect`, which is the call.
            Edit,
            Back,
        }

        fn id(screen: Screen) -> u8 {
            match screen {
                Screen::Home => 1,
                Screen::List => 2,
                Screen::Connect => 3,
                Screen::Register => 4,
                Screen::Edit => 5,
                Screen::Back => 6,
            }
        }

        /// A screen with no subject.
        pub fn page(screen: Screen) -> [u8; 2] {
            [0xF0, id(screen)]
        }

        /// A screen about one application.
        ///
        /// The `client_id` is appended as its own text rather than packed into bits: it
        /// is 36 characters, the limit is 100, and the readable form is one a test can
        /// write down and a person can recognise in a log.
        pub fn application(screen: Screen, client_id: &str) -> Vec<u8> {
            let mut out = page(screen).to_vec();
            out.extend_from_slice(client_id.as_bytes());

            out
        }

        /// The string a component carries, packed the way every other id in this service
        /// is packed, so the dispatcher can read it back with [`parse`].
        pub fn custom_id(screen: Screen) -> String {
            crate::custom_id::encode(0, &page(screen))
        }

        pub fn parse(source: &[u8]) -> Result<(Screen, String), UiError> {
            let (id, data) = parse_id(source)?;

            let screen = match id {
                1 => Screen::Home,
                2 => Screen::List,
                3 => Screen::Connect,
                4 => Screen::Register,
                5 => Screen::Edit,
                6 => Screen::Back,
                _ => return Err(UiError::Unknown(id)),
            };

            // The packing left-aligns the last group, so a decoded payload comes back
            // padded with NULs — the module above says so. A `client_id` is a uuid and
            // never contains one, so cutting them is what recovers the string that went
            // in, and a payload that is not text at all is one this module did not build.
            let trimmed = data
                .iter()
                .rposition(|byte| *byte != 0)
                .map_or(&data[..0], |end| &data[..=end]);

            let client_id = String::from_utf8(trimmed.to_vec()).map_err(|_| UiError::Head)?;

            Ok((screen, client_id))
        }
    }

    pub mod modal {
        use super::{UiError, parse_id};

        pub fn confirm_currency_delete() -> [u8; 2] {
            [0xF0, 1]
        }

        pub fn parse(source: &[u8]) -> Result<([&'static str; 2], Vec<u8>), UiError> {
            let (id, data) = parse_id(source)?;

            match id {
                1 => Ok((["delete", "confirm"], data)),
                _ => Err(UiError::Unknown(id)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `VirtualCryptoWeb.CustomIdTest`: every three-byte payload round-trips with
    /// one padding byte, and five bytes with four.
    #[test]
    fn short_payloads_round_trip_with_padding() {
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                for c in 0..=127u8 {
                    let encoded = encode(0, &[a, b, c]);

                    assert_eq!(parse(&encoded), vec![a, b, c, 0], "a={a} b={b} c={c}");
                }
            }
        }

        assert_eq!(
            parse(&encode(0, &[255, 255, 255, 255, 255])),
            vec![255, 255, 255, 255, 255, 0, 0, 0, 0]
        );
    }

    #[test]
    fn the_discriminator_is_dropped() {
        let encoded = encode(7, &[1, 2, 3]);

        assert_eq!(parse(&encoded)[..3], [1, 2, 3]);
    }

    #[test]
    fn a_button_path_survives_the_round_trip() {
        let encoded = encode(0, &button_id());
        let (path, _) = ui::button::parse(&parse(&encoded)).expect("a known path");

        assert_eq!(
            path,
            ui::button::Path::List(ui::button::ListScope::Received)
        );

        fn button_id() -> [u8; 2] {
            ui::button::claim_list(ui::button::ListScope::Received)
        }
    }

    #[test]
    fn an_unknown_discriminator_is_rejected() {
        assert_eq!(ui::button::parse(&[0x00, 1]), Err(UiError::Head));
    }

    /// The screen and its subject both survive, which is the whole reason they travel
    /// here: Discord sends the `custom_id` back and nothing else, and an ephemeral
    /// message cannot be fetched to ask what it was showing.
    #[test]
    fn a_developer_screen_survives_the_round_trip() {
        let client_id = "3f1c2f4e-9a11-4d2b-8c3e-5f6a7b8c9d0e";

        let encoded = encode(
            0,
            &ui::developer::application(ui::developer::Screen::Connect, client_id),
        );
        let (screen, found) = ui::developer::parse(&parse(&encoded)).expect("a known screen");

        assert_eq!(screen, ui::developer::Screen::Connect);
        assert_eq!(found, client_id);
    }

    /// A screen with no subject is the same shape with nothing after the id.
    #[test]
    fn a_developer_screen_without_a_subject_round_trips() {
        let encoded = encode(0, &ui::developer::page(ui::developer::Screen::Home));
        let (screen, found) = ui::developer::parse(&parse(&encoded)).expect("a known screen");

        assert_eq!(screen, ui::developer::Screen::Home);
        assert_eq!(found, "");
    }

    /// Discord cuts a `custom_id` at 100 characters, and the largest one here is a screen
    /// plus a uuid. Asserted rather than assumed, because the packing is what makes it
    /// fit and the packing is easy to change.
    #[test]
    fn the_longest_developer_id_fits_in_discords_hundred_characters() {
        let client_id = "3f1c2f4e-9a11-4d2b-8c3e-5f6a7b8c9d0e";
        let encoded = encode(
            0,
            &ui::developer::application(ui::developer::Screen::Connect, client_id),
        );

        assert!(
            encoded.chars().count() <= 100,
            "{} chars",
            encoded.chars().count()
        );
    }

    #[test]
    fn an_unknown_developer_screen_is_rejected() {
        assert_eq!(ui::developer::parse(&[0xF0, 99]), Err(UiError::Unknown(99)));
    }

    #[test]
    fn an_unknown_id_is_rejected() {
        assert_eq!(ui::button::parse(&[0xF0, 99]), Err(UiError::Unknown(99)));
    }
}
