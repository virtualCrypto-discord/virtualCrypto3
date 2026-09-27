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
        }

        /// What a claim's button is about. The numbers are the space's own and the two
        /// sets are not the same one: `Act` is a list row's action, which carries the list
        /// options after the id, and `ActionSingle` is a claim's own screen, which carries
        /// the id and nothing else.
        ///
        /// `Back` was here — the selection screen's way out, and the selection itself — and
        /// the numbers it had are gone rather than reused: a message already in a DM carries
        /// them, and an id this service no longer issues should be refused, not read as
        /// something else.
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
            };

            [0xF0, id]
        }

        pub fn claim_action_single(action: Action) -> [u8; 2] {
            let id = match action {
                Action::Approve => 8,
                Action::Deny => 9,
                Action::Cancel => 10,
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
                8 => Path::ActionSingle(Action::Approve),
                9 => Path::ActionSingle(Action::Deny),
                10 => Path::ActionSingle(Action::Cancel),
                _ => return Err(UiError::Unknown(id)),
            };

            Ok((path, data))
        }
    }

    /// The arrows a balance list is paged with.
    ///
    /// Not from the Elixir — its `/bal` answered one fence with every currency in it — so
    /// what this mirrors is the shape of the spaces beside it, and a head byte of its own for
    /// the contract space's reason: these ids carry a page number, and a parser that only
    /// accepts its own head refuses every other space's bytes rather than reading them as its
    /// own.
    pub mod bal {
        use super::UiError;

        const HEAD: u8 = 0xF5;

        /// The head this space writes, so a test and a screen agree on it.
        pub fn head() -> u8 {
            HEAD
        }

        /// Where a pagination button moves the list: first, previous, next and last are what
        /// its arrows say, and a cursor cannot say "back".
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Page {
            First,
            Previous,
            Next,
            Last,
        }

        fn id(page: Page) -> u8 {
            match page {
                Page::First => 1,
                Page::Previous => 2,
                Page::Next => 3,
                Page::Last => 4,
            }
        }

        /// The string a pagination button carries: which move, then where to.
        pub fn page_custom_id(page: Page, number: i64) -> String {
            let mut data = vec![HEAD, id(page)];
            data.extend_from_slice(number.to_string().as_bytes());

            crate::custom_id::encode(0, &data)
        }

        pub fn parse(source: &[u8]) -> Result<(Page, i64), UiError> {
            let [head, id, rest @ ..] = source else {
                return Err(UiError::Head);
            };

            if *head != HEAD {
                return Err(UiError::Head);
            }

            let page = match id {
                1 => Page::First,
                2 => Page::Previous,
                3 => Page::Next,
                4 => Page::Last,
                other => return Err(UiError::Unknown(u16::from(*other))),
            };

            // The packing left-aligns the last group, so a decoded payload comes back padded
            // with NULs — the module above says so. A number never contains one, which is what
            // makes cutting them the inverse of what went in.
            let trimmed = rest
                .iter()
                .rposition(|byte| *byte != 0)
                .map_or(&rest[..0], |end| &rest[..=end]);
            let text = String::from_utf8(trimmed.to_vec()).map_err(|_| UiError::Head)?;

            Ok((page, text.parse().map_err(|_| UiError::Head)?))
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
        use super::UiError;

        /// This space's own head byte, and the reason it has one.
        ///
        /// Every other space here shares `0xF0` and is told apart by *which parser runs*,
        /// which works while each id shape is used in one place. This one is not: it shares
        /// screens with `ui::modal`'s delete confirmation, and the first version of it took
        /// `[0xF0, 1]` for `Home` — the same bytes as `confirm_currency_delete` — so a
        /// dispatcher that tried this space first stole that modal.
        ///
        /// A head of its own means the bytes say which space they are, and a parse that
        /// only accepts them refuses the rest.
        const HEAD: u8 = 0xF1;

        fn parse_id(source: &[u8]) -> Result<(u8, Vec<u8>), UiError> {
            match source {
                [head, id, rest @ ..] if *head == HEAD => Ok((*id, rest.to_vec())),
                _ => Err(UiError::Head),
            }
        }

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
            Show,
            RotateSecret,
            ConfirmConnect,
        }

        fn id(screen: Screen) -> u8 {
            match screen {
                Screen::Home => 1,
                Screen::List => 2,
                Screen::Connect => 3,
                Screen::Register => 4,
                Screen::Edit => 5,
                Screen::Back => 6,
                Screen::Show => 7,
                Screen::RotateSecret => 8,
                Screen::ConfirmConnect => 10,
            }
        }

        /// The head this space writes, so the tests and the screens agree on it.
        pub fn head() -> u8 {
            HEAD
        }

        /// A screen with no subject.
        pub fn page(screen: Screen) -> [u8; 2] {
            [HEAD, id(screen)]
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

        /// The field a component edits, when the id was built by [`custom_id_for_field`].
        ///
        /// A newline separates it from the `client_id` because neither a uuid nor a field's
        /// name can contain one, and an id that names no field splits to nothing.
        pub fn field_of(data: &str) -> (&str, &str) {
            match data.split_once('\n') {
                Some((client_id, field)) => (client_id, field),
                None => (data, ""),
            }
        }

        /// The string a component carries for one field of one application.
        ///
        /// The field is in the id because there are nine of them and each has its own control:
        /// a submission has to say which of the nine it is, and Discord hands back only what the
        /// component was built with.
        pub fn custom_id_for_field(screen: Screen, client_id: &str, field: &str) -> String {
            let mut data = application(screen, client_id);
            data.push(b'\n');
            data.extend_from_slice(field.as_bytes());

            crate::custom_id::encode(0, &data)
        }

        /// The string a component carries for a screen about one application.
        ///
        /// The `client_id` has to be in the id and not somewhere else: a form that changes
        /// an application is opened from a button on that application's screen, and the
        /// submission comes back with nothing but the id it was opened with.
        pub fn custom_id_for(screen: Screen, client_id: &str) -> String {
            crate::custom_id::encode(0, &application(screen, client_id))
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
                7 => Screen::Show,
                8 => Screen::RotateSecret,
                10 => Screen::ConfirmConnect,
                _ => return Err(UiError::Unknown(u16::from(id))),
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

    /// The buttons a contract's own screen offers: answering it, or taking the
    /// delegation back.
    ///
    /// Not from the Elixir — its contract page is a mockup with no `phx-click` on
    /// either of its buttons — so what this mirrors is the shape of the spaces
    /// beside it, and a head byte of its own for the developer space's reason:
    /// these ids carry a contract id after the action, and a parser that only
    /// accepts its own head refuses every other space's bytes rather than reading
    /// them as its own.
    pub mod contract {
        use super::UiError;

        const HEAD: u8 = 0xF2;

        /// The head this space writes, so a test and a screen agree on it.
        pub fn head() -> u8 {
            HEAD
        }

        /// Which of the three things a party can do about a contract.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Action {
            Approve,
            Refuse,
            Withdraw,
        }

        /// Where a pagination button moves the list, whose page numbers are what
        /// the screen's arrows are: a cursor cannot say "back".
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Page {
            First,
            Previous,
            Next,
            Last,
        }

        /// What a button in this space is about: an answer about one contract, or a
        /// move from one page of the list to another.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Pressed {
            Decided(Action, i64),
            Paged(Page, i64),
        }

        fn id(action: Action) -> u8 {
            match action {
                Action::Approve => 1,
                Action::Refuse => 2,
                Action::Withdraw => 3,
            }
        }

        fn page_id(page: Page) -> u8 {
            match page {
                Page::First => 4,
                Page::Previous => 5,
                Page::Next => 6,
                Page::Last => 7,
            }
        }

        /// The string a button carries: the action, then the contract it is about.
        ///
        /// The id travels as its own text rather than packed into bits, as the
        /// developer space's `client_id` does: it is at most nineteen digits, the
        /// limit is a hundred characters, and the readable form is one a test can
        /// write down and a person can recognise in a log.
        pub fn custom_id(action: Action, contract_id: i64) -> String {
            let mut data = vec![HEAD, id(action)];
            data.extend_from_slice(contract_id.to_string().as_bytes());

            crate::custom_id::encode(0, &data)
        }

        /// The string a pagination button carries: which move, then where to.
        ///
        /// The same shape as a decision's, because it is the same space: the number
        /// is a page here and a contract there, and the action says which.
        pub fn page_custom_id(page: Page, number: i64) -> String {
            let mut data = vec![HEAD, page_id(page)];
            data.extend_from_slice(number.to_string().as_bytes());

            crate::custom_id::encode(0, &data)
        }

        pub fn parse(source: &[u8]) -> Result<Pressed, UiError> {
            let [head, id, rest @ ..] = source else {
                return Err(UiError::Head);
            };

            if *head != HEAD {
                return Err(UiError::Head);
            }

            let read = match id {
                1 => |number| Pressed::Decided(Action::Approve, number),
                2 => |number| Pressed::Decided(Action::Refuse, number),
                3 => |number| Pressed::Decided(Action::Withdraw, number),
                4 => |number| Pressed::Paged(Page::First, number),
                5 => |number| Pressed::Paged(Page::Previous, number),
                6 => |number| Pressed::Paged(Page::Next, number),
                7 => |number| Pressed::Paged(Page::Last, number),
                other => return Err(UiError::Unknown(u16::from(*other))),
            };

            // The packing left-aligns the last group, so a decoded payload comes
            // back padded with NULs — the module above says so. A number never
            // contains one, which is what makes cutting them the inverse of what
            // went in.
            let trimmed = rest
                .iter()
                .rposition(|byte| *byte != 0)
                .map_or(&rest[..0], |end| &rest[..=end]);

            let text = String::from_utf8(trimmed.to_vec()).map_err(|_| UiError::Head)?;
            let number = text.parse().map_err(|_| UiError::Head)?;

            Ok(read(number))
        }
    }

    /// The buttons a guild's `/grant` screen offers: taking an application's
    /// permission back, or moving between pages of the list.
    ///
    /// Not from the Elixir — it had no list of what a guild had allowed and no
    /// call that took a permission away — so what this mirrors is the shape of
    /// the spaces beside it, and a head byte of its own for the contract space's
    /// reason: these ids carry a `client_id` after the action, and a parser that
    /// only accepts its own head refuses every other space's bytes rather than
    /// reading them as its own.
    pub mod grant {
        use super::UiError;

        const HEAD: u8 = 0xF4;

        /// The head this space writes, so a test and a screen agree on it.
        pub fn head() -> u8 {
            HEAD
        }

        /// Where a pagination button moves the list, which is the contract
        /// list's four moves rather than a cursor: a cursor cannot say "back".
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Page {
            First,
            Previous,
            Next,
            Last,
        }

        /// What a button in this space is about: one application's permission, or
        /// a move from one page of the list to another.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum Pressed {
            Revoked(String),
            Confirmed(i64),
            ReviewPage(i64, i64),
            Details(i64, i64),
            RevokeOne(i64),
            UserRevoked(i64, uuid::Uuid),
            UserPaged(i64, i64),
            Paged(Page, i64),
        }

        fn page_id(page: Page) -> u8 {
            match page {
                Page::First => 2,
                Page::Previous => 3,
                Page::Next => 4,
                Page::Last => 5,
            }
        }

        /// The string the revoke button carries: the action, then the application
        /// it is about.
        ///
        /// The `client_id` travels as its own text rather than packed into bits,
        /// as the developer space's does: it is 36 characters, the limit is a
        /// hundred, and the readable form is one a test can write down and a
        /// person can recognise in a log.
        pub fn revoke_custom_id(client_id: &str) -> String {
            let mut data = vec![HEAD, 1];
            data.extend_from_slice(client_id.as_bytes());

            crate::custom_id::encode(0, &data)
        }

        /// The string a pagination button carries: which move, then where to.
        ///
        /// The same shape as a revoke's, because it is the same space: the text
        /// is a page number here and a `client_id` there, and the id says which.
        pub fn page_custom_id(page: Page, number: i64) -> String {
            let mut data = vec![HEAD, page_id(page)];
            data.extend_from_slice(number.to_string().as_bytes());

            crate::custom_id::encode(0, &data)
        }

        pub fn confirm_custom_id(request: i64) -> String {
            encoded(6, &request.to_string())
        }
        pub fn user_revoke_custom_id(user: i64, client: &str) -> String {
            encoded(7, &format!("{user}:{client}"))
        }
        pub fn user_page_custom_id(user: i64, page: i64) -> String {
            encoded(8, &format!("{user}:{page}"))
        }
        pub fn review_page_custom_id(request: i64, page: i64) -> String {
            encoded(9, &format!("{request}:{page}"))
        }
        pub fn details_custom_id(grant: i64, page: i64) -> String {
            encoded(10, &format!("{grant}:{page}"))
        }
        pub fn revoke_one_custom_id(grant: i64) -> String {
            encoded(11, &grant.to_string())
        }
        fn encoded(action: u8, payload: &str) -> String {
            let mut data = vec![HEAD, action];
            data.extend_from_slice(payload.as_bytes());
            crate::custom_id::encode(0, &data)
        }

        pub fn parse(source: &[u8]) -> Result<Pressed, UiError> {
            let [head, id, rest @ ..] = source else {
                return Err(UiError::Head);
            };

            if *head != HEAD {
                return Err(UiError::Head);
            }

            // The packing left-aligns the last group, so a decoded payload comes
            // back padded with NULs — the module above says so. Neither a uuid nor
            // a number contains one, which is what makes cutting them the inverse
            // of what went in.
            let trimmed = rest
                .iter()
                .rposition(|byte| *byte != 0)
                .map_or(&rest[..0], |end| &rest[..=end]);
            let text = || String::from_utf8(trimmed.to_vec()).map_err(|_| UiError::Head);

            match id {
                1 => Ok(Pressed::Revoked(text()?)),
                2..=5 => {
                    let page = match id {
                        2 => Page::First,
                        3 => Page::Previous,
                        4 => Page::Next,
                        _ => Page::Last,
                    };

                    Ok(Pressed::Paged(
                        page,
                        text()?.parse().map_err(|_| UiError::Head)?,
                    ))
                }
                6 => Ok(Pressed::Confirmed(
                    text()?.parse().map_err(|_| UiError::Head)?,
                )),
                11 => Ok(Pressed::RevokeOne(
                    text()?.parse().map_err(|_| UiError::Head)?,
                )),
                9 | 10 => {
                    let value = text()?;
                    let (key, page) = value.split_once(':').ok_or(UiError::Head)?;
                    let key = key.parse().map_err(|_| UiError::Head)?;
                    let page = page.parse().map_err(|_| UiError::Head)?;
                    Ok(if *id == 9 {
                        Pressed::ReviewPage(key, page)
                    } else {
                        Pressed::Details(key, page)
                    })
                }
                7 | 8 => {
                    let value = text()?;
                    let (user, value) = value.split_once(':').ok_or(UiError::Head)?;
                    let user = user.parse().map_err(|_| UiError::Head)?;
                    if *id == 7 {
                        Ok(Pressed::UserRevoked(
                            user,
                            value.parse().map_err(|_| UiError::Head)?,
                        ))
                    } else {
                        Ok(Pressed::UserPaged(
                            user,
                            value.parse().map_err(|_| UiError::Head)?,
                        ))
                    }
                }
                other => Err(UiError::Unknown(u16::from(*other))),
            }
        }
    }

    /// The mutes screen: the arrows that move it, and the button each row carries.
    ///
    /// A head byte of its own, for the contract space's reason: an id here carries the target
    /// of a mute after the action, and a parser that only accepts its own head refuses the
    /// other spaces' bytes rather than reading them as its own.
    pub mod mute {
        use super::UiError;

        const HEAD: u8 = 0xF6;

        /// The head this space writes, so a test and a screen agree on it.
        pub fn head() -> u8 {
            HEAD
        }

        /// Where a pagination button moves the list, which is the four moves the lists beside
        /// it have rather than a cursor: a cursor cannot say "back".
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Page {
            First,
            Previous,
            Next,
            Last,
        }

        /// What a button in this space is about.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum Pressed {
            /// A currency's mute, by the id the row was built from.
            Currency(i64),
            /// Somebody's mute, by the Discord id the row shows.
            User(i64),
            /// A move from one page of the list to another.
            Paged(Page, i64),
        }

        /// The string an unmute button carries: what kind of target, then which one.
        ///
        /// The target only, and not the person the mute belongs to: the reader a press unmutes
        /// is the one who pressed, because a mute is the reader's own. A button sent by somebody
        /// else is therefore a button that unmutes *their* mute of the same thing, which is
        /// what makes this id harmless in the hands of anybody who has seen it.
        pub fn unmute_custom_id(target: &vc_core::mute::Target) -> String {
            match target {
                vc_core::mute::Target::Currency { id, .. } => encoded(1, &id.to_string()),
                vc_core::mute::Target::User { discord_id } => encoded(2, &discord_id.to_string()),
            }
        }

        /// The string a pagination button carries: which move, then where to.
        pub fn page_custom_id(page: Page, number: i64) -> String {
            let action = match page {
                Page::First => 3,
                Page::Previous => 4,
                Page::Next => 5,
                Page::Last => 6,
            };

            encoded(action, &number.to_string())
        }

        fn encoded(action: u8, payload: &str) -> String {
            let mut data = vec![HEAD, action];
            data.extend_from_slice(payload.as_bytes());

            crate::custom_id::encode(0, &data)
        }

        pub fn parse(source: &[u8]) -> Result<Pressed, UiError> {
            let [head, id, rest @ ..] = source else {
                return Err(UiError::Head);
            };

            if *head != HEAD {
                return Err(UiError::Head);
            }

            // The packing left-aligns the last group, so a decoded payload comes back padded
            // with NULs — the grant space says so above. Neither an id nor a number contains
            // one, which is what makes cutting them the inverse of what went in.
            let trimmed = rest
                .iter()
                .rposition(|byte| *byte != 0)
                .map_or(&rest[..0], |end| &rest[..=end]);
            let number = || -> Result<i64, UiError> {
                String::from_utf8(trimmed.to_vec())
                    .map_err(|_| UiError::Head)?
                    .parse()
                    .map_err(|_| UiError::Head)
            };

            match id {
                1 => Ok(Pressed::Currency(number()?)),
                2 => Ok(Pressed::User(number()?)),
                3..=6 => {
                    let page = match id {
                        3 => Page::First,
                        4 => Page::Previous,
                        5 => Page::Next,
                        _ => Page::Last,
                    };

                    Ok(Pressed::Paged(page, number()?))
                }
                other => Err(UiError::Unknown(u16::from(*other))),
            }
        }
    }

    /// The history screens: the arrows that move one page of a ledger.
    ///
    /// A head byte of its own, for the contract space's reason: an id here carries which ledger
    /// it is and what that ledger is narrowed to, and a parser that only accepts its own head
    /// refuses the other spaces' bytes rather than reading them as its own.
    pub mod history {
        use super::UiError;

        const HEAD: u8 = 0xF7;

        /// The head this space writes, so a test and a screen agree on it.
        pub fn head() -> u8 {
            HEAD
        }

        /// Which ledger a screen shows: what one person paid and was paid, or what one guild's
        /// pool issued.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Screen {
            Paid,
            Issued,
        }

        /// A screen, the page it is on, and what it is narrowed to.
        ///
        /// The filters travel with the page because Discord sends the `custom_id` back and
        /// nothing else: an arrow that forgot the filter would answer a narrowed list with an
        /// unnarrowed page.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct Listing {
            pub screen: Screen,
            pub page: i64,
            /// The currency's unit, when the screen is narrowed to one.
            pub unit: Option<String>,
            /// The Discord id on the other side, when the screen is narrowed to one person.
            pub discord_id: Option<i64>,
        }

        /// The string a pagination button carries: `kind:page:unit:discord id`.
        ///
        /// The first two and last separators delimit the fields: imported units may contain
        /// colons themselves. An absent filter is an empty field.
        pub fn page_custom_id(listing: &Listing) -> String {
            let kind = match listing.screen {
                Screen::Paid => "paid",
                Screen::Issued => "issued",
            };

            let unit = listing.unit.as_deref().unwrap_or_default();
            let discord_id = listing
                .discord_id
                .map(|id| id.to_string())
                .unwrap_or_default();

            let mut data = vec![HEAD];
            data.extend_from_slice(
                format!("{kind}:{}:{unit}:{discord_id}", listing.page).as_bytes(),
            );

            crate::custom_id::encode(0, &data)
        }

        pub fn parse(source: &[u8]) -> Result<Listing, UiError> {
            let [head, rest @ ..] = source else {
                return Err(UiError::Head);
            };

            if *head != HEAD {
                return Err(UiError::Head);
            }

            // The packing left-aligns the last group, so a decoded payload comes back padded
            // with NULs — the grant space says so above. None of the four fields holds one,
            // which is what makes cutting the text the inverse of what went in.
            let trimmed = rest
                .iter()
                .rposition(|byte| *byte != 0)
                .map_or(&rest[..0], |end| &rest[..=end]);
            let text = String::from_utf8(trimmed.to_vec()).map_err(|_| UiError::Head)?;
            let mut fields = text.splitn(3, ':');

            let screen = match fields.next() {
                Some("paid") => Screen::Paid,
                Some("issued") => Screen::Issued,
                _ => return Err(UiError::Head),
            };

            let page = fields
                .next()
                .and_then(|page| page.parse().ok())
                .ok_or(UiError::Head)?;
            let (unit, discord_id) = fields
                .next()
                .and_then(|filters| filters.rsplit_once(':'))
                .ok_or(UiError::Head)?;
            let unit = (!unit.is_empty()).then(|| unit.to_owned());
            let discord_id = match discord_id {
                "" => None,
                id => Some(id.parse().map_err(|_| UiError::Head)?),
            };

            Ok(Listing {
                screen,
                page,
                unit,
                discord_id,
            })
        }
    }

    /// The menu `/help` offers and the button that goes back to its list.
    ///
    /// A head byte of its own, for the developer space's reason: the dispatcher's
    /// generic string-select arm belongs to the claim list, so an id that did not
    /// say which space it was would be read as one of that list's — and a menu
    /// choice is the one thing `/help` has that is sent as a string select.
    pub mod help {
        use super::UiError;

        const HEAD: u8 = 0xF3;

        /// Which of the components an id belongs to. The numbers are this space's, and they
        /// must not be renumbered: a message already in a DM carries them.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Screen {
            /// The menu a command is chosen from.
            Select,
            /// The button that goes back to the list of commands.
            Index,
            /// The button that opens 「はじめに」 — the page, not a link to it.
            Start,
        }

        /// The head this space writes, so a test and a screen agree on it.
        pub fn head() -> u8 {
            HEAD
        }

        fn id(screen: Screen) -> u8 {
            match screen {
                Screen::Select => 1,
                Screen::Index => 2,
                Screen::Start => 3,
            }
        }

        /// There is nothing after the id: what the choice *is* arrives as the
        /// select's own values, so this id only has to say what was pressed.
        fn custom_id(screen: Screen) -> String {
            crate::custom_id::encode(0, &[HEAD, id(screen)])
        }

        pub fn select() -> String {
            custom_id(Screen::Select)
        }

        pub fn index() -> String {
            custom_id(Screen::Index)
        }

        pub fn start() -> String {
            custom_id(Screen::Start)
        }

        pub fn parse(source: &[u8]) -> Result<Screen, UiError> {
            match source {
                [head, id, ..] if *head == HEAD => match id {
                    1 => Ok(Screen::Select),
                    2 => Ok(Screen::Index),
                    3 => Ok(Screen::Start),
                    other => Err(UiError::Unknown(u16::from(*other))),
                },
                _ => Err(UiError::Head),
            }
        }
    }

    /// Personal token list buttons. The owner is checked against the interaction user;
    /// revocation carries an immutable token id so stale screens cannot target a replacement.
    pub mod pat {
        use super::UiError;
        use uuid::Uuid;

        const HEAD: u8 = 0xF8;

        pub fn head() -> u8 {
            HEAD
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Pressed {
            Page {
                owner: i64,
                number: usize,
            },
            Revoke {
                owner: i64,
                number: usize,
                token_id: Uuid,
            },
        }

        pub fn page(owner: i64, number: usize) -> String {
            encode(1, &format!("{owner}:{number}"))
        }

        pub fn revoke(owner: i64, number: usize, token_id: Uuid) -> String {
            encode(2, &format!("{owner}:{number}:{token_id}"))
        }

        fn encode(action: u8, payload: &str) -> String {
            let mut data = vec![HEAD, action];
            data.extend_from_slice(payload.as_bytes());
            crate::custom_id::encode(0, &data)
        }

        pub fn parse(source: &[u8]) -> Result<Pressed, UiError> {
            let [HEAD, action, rest @ ..] = source else {
                return Err(UiError::Head);
            };
            let payload = std::str::from_utf8(rest).map_err(|_| UiError::Head)?;
            let mut fields = payload.trim_end_matches('\0').split(':');
            let owner = fields
                .next()
                .and_then(|s| s.parse().ok())
                .filter(|id| *id > 0)
                .ok_or(UiError::Head)?;
            let number = fields
                .next()
                .and_then(|s| s.parse().ok())
                .filter(|n| *n > 0)
                .ok_or(UiError::Head)?;
            let pressed = match action {
                1 => Pressed::Page { owner, number },
                2 => Pressed::Revoke {
                    owner,
                    number,
                    token_id: fields
                        .next()
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .ok_or(UiError::Head)?,
                },
                other => return Err(UiError::Unknown(u16::from(*other))),
            };
            if fields.next().is_some() {
                return Err(UiError::Head);
            }
            Ok(pressed)
        }
    }

    #[cfg(test)]
    mod pat_tests {
        use super::pat::{self, Pressed};

        #[test]
        fn buttons_round_trip_within_discord_limit() {
            let owner = i64::MAX;
            let token_id = uuid::Uuid::new_v4();
            for (id, expected) in [
                (
                    pat::page(owner, usize::MAX),
                    Pressed::Page {
                        owner,
                        number: usize::MAX,
                    },
                ),
                (
                    pat::revoke(owner, 3, token_id),
                    Pressed::Revoke {
                        owner,
                        number: 3,
                        token_id,
                    },
                ),
            ] {
                assert!(id.encode_utf16().count() <= 100);
                assert_eq!(pat::parse(&crate::custom_id::parse(&id)), Ok(expected));
            }
        }

        #[test]
        fn malformed_ids_are_refused() {
            for bytes in [
                &b"\xf7\x011:1"[..],
                &b"\xf8"[..],
                &b"\xf8\x011:0"[..],
                &b"\xf8\x010:1"[..],
                &b"\xf8\x011:1:extra"[..],
                &b"\xf8\x021:1:invalid-uuid"[..],
                &b"\xf8\x091:1"[..],
            ] {
                assert!(pat::parse(bytes).is_err());
            }
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

    /// The other spaces' head is refused, which is the whole point of this one having its
    /// own: `[0xF0, 1]` is the delete confirmation's, and reading it here once stole that
    /// modal.
    #[test]
    fn the_other_spaces_head_is_not_this_one() {
        assert_eq!(
            ui::developer::parse(&[0xF0, 1]),
            Err(UiError::Head),
            "0xF0 belongs to every other space"
        );
        assert!(ui::developer::parse(&[ui::developer::head(), 1]).is_ok());
    }

    #[test]
    fn an_unknown_developer_screen_is_rejected() {
        assert_eq!(
            ui::developer::parse(&[ui::developer::head(), 99]),
            Err(UiError::Unknown(99))
        );
    }

    #[test]
    fn an_unknown_id_is_rejected() {
        assert_eq!(ui::button::parse(&[0xF0, 99]), Err(UiError::Unknown(99)));
    }

    /// The action and the contract both survive, which is the whole reason they
    /// travel here: Discord sends the `custom_id` back and nothing else.
    #[test]
    fn a_contract_button_survives_the_round_trip() {
        let encoded = ui::contract::custom_id(ui::contract::Action::Withdraw, 12_345);
        let pressed = ui::contract::parse(&parse(&encoded)).expect("a known action");

        assert_eq!(
            pressed,
            ui::contract::Pressed::Decided(ui::contract::Action::Withdraw, 12_345)
        );
    }

    /// And a page's number survives the same way, in the same space: what tells
    /// them apart is the action byte, which is why the number may mean a contract
    /// or a page.
    #[test]
    fn a_contract_page_survives_the_round_trip() {
        let encoded = ui::contract::page_custom_id(ui::contract::Page::Last, 3);
        let pressed = ui::contract::parse(&parse(&encoded)).expect("a known page");

        assert_eq!(
            pressed,
            ui::contract::Pressed::Paged(ui::contract::Page::Last, 3)
        );
    }

    /// Its own head is what keeps it out of the other spaces, and theirs out of
    /// it — `0xF1` is the developer screens' and `0xF0` everything else.
    #[test]
    fn another_spaces_head_is_not_a_contracts() {
        assert_eq!(ui::contract::parse(&[0xF0, 1, b'7']), Err(UiError::Head));
        assert_eq!(
            ui::contract::parse(&[ui::developer::head(), 1, b'7']),
            Err(UiError::Head)
        );
        assert_eq!(
            ui::contract::parse(&[ui::contract::head(), 1, b'7']),
            Ok(ui::contract::Pressed::Decided(
                ui::contract::Action::Approve,
                7
            ))
        );
    }

    #[test]
    fn an_unknown_contract_action_is_rejected() {
        assert_eq!(
            ui::contract::parse(&[ui::contract::head(), 99, b'1']),
            Err(UiError::Unknown(99))
        );
    }

    /// `/help`'s two components, which carry nothing but which they are.
    #[test]
    fn a_help_component_survives_the_round_trip() {
        assert_eq!(
            ui::help::parse(&parse(&ui::help::select())),
            Ok(ui::help::Screen::Select)
        );
        assert_eq!(
            ui::help::parse(&parse(&ui::help::index())),
            Ok(ui::help::Screen::Index)
        );

        // The limit is a hundred characters, and this is two: asserted anyway,
        // because an id that grew a payload should have to say so here.
        assert!(ui::help::select().chars().count() <= 100);
    }

    /// Its own head, which is what keeps a choice on `/help` out of the claim
    /// list's select arm — that arm is the fallback every string select reaches,
    /// so an id without one would be read as a claim's.
    #[test]
    fn another_spaces_head_is_not_helps() {
        assert_eq!(ui::help::parse(&[0xF0, 1]), Err(UiError::Head));
        assert_eq!(
            ui::help::parse(&[ui::developer::head(), 1]),
            Err(UiError::Head)
        );
        assert_eq!(
            ui::help::parse(&[ui::contract::head(), 1]),
            Err(UiError::Head)
        );
        assert_eq!(
            ui::help::parse(&[ui::help::head(), 9]),
            Err(UiError::Unknown(9))
        );
    }

    /// What the guild's screen would show and what its button says have to
    /// survive the round trip together: Discord sends the `custom_id` back and
    /// nothing else, and the revoke is about exactly one application.
    #[test]
    fn a_grant_button_survives_the_round_trip() {
        let client_id = "3f1c2f4e-9a11-4d2b-8c3e-5f6a7b8c9d0e";
        let encoded = ui::grant::revoke_custom_id(client_id);
        let pressed = ui::grant::parse(&parse(&encoded)).expect("a known action");

        assert_eq!(pressed, ui::grant::Pressed::Revoked(client_id.to_owned()));
        assert!(
            encoded.chars().count() <= 100,
            "{} chars",
            encoded.chars().count()
        );
    }

    /// And a page's number survives the same way, in the same space: what tells
    /// them apart is the id byte, which is why the text may mean an application
    /// or a page.
    #[test]
    fn a_grant_page_survives_the_round_trip() {
        let encoded = ui::grant::page_custom_id(ui::grant::Page::Last, 3);
        let pressed = ui::grant::parse(&parse(&encoded)).expect("a known page");

        assert_eq!(pressed, ui::grant::Pressed::Paged(ui::grant::Page::Last, 3));
    }

    /// Its own head is what keeps it out of the other spaces of a guild's
    /// screen, and theirs out of it: the contract buttons carry a number where
    /// this one carries a `client_id`.
    #[test]
    fn another_spaces_head_is_not_a_grants() {
        assert_eq!(ui::grant::parse(&[0xF0, 1, b'7']), Err(UiError::Head));
        assert_eq!(
            ui::grant::parse(&[ui::contract::head(), 1, b'7']),
            Err(UiError::Head)
        );
        assert_eq!(
            ui::grant::parse(&[ui::grant::head(), 12, b'7']),
            Err(UiError::Unknown(12))
        );
    }

    /// What a row is about has to survive the round trip: a currency by its id and a person by
    /// their Discord id, which is what the screen drew beside the button.
    #[test]
    fn a_mute_button_survives_the_round_trip() {
        let currency = vc_core::mute::Target::Currency {
            id: 12,
            unit: "n".to_string(),
            name: "nyan".to_string(),
        };

        assert_eq!(
            ui::mute::parse(&parse(&ui::mute::unmute_custom_id(&currency))),
            Ok(ui::mute::Pressed::Currency(12))
        );

        let person = vc_core::mute::Target::User {
            discord_id: 100_000_000_000_000_001,
        };
        let encoded = ui::mute::unmute_custom_id(&person);

        assert_eq!(
            ui::mute::parse(&parse(&encoded)),
            Ok(ui::mute::Pressed::User(100_000_000_000_000_001))
        );
        assert!(
            encoded.chars().count() <= 100,
            "{} chars",
            encoded.chars().count()
        );
    }

    /// And a page's number survives the same way, in the same space: what tells them apart is
    /// the action byte, which is why a number here may mean a target or a page.
    #[test]
    fn a_mute_page_survives_the_round_trip() {
        let encoded = ui::mute::page_custom_id(ui::mute::Page::Last, 3);
        let pressed = ui::mute::parse(&parse(&encoded)).expect("a known page");

        assert_eq!(pressed, ui::mute::Pressed::Paged(ui::mute::Page::Last, 3));
    }

    /// Its own head, for every other space's reason: the mutes screen's ids carry an id after
    /// the action, and `0xF0` is every other space's.
    #[test]
    fn another_spaces_head_is_not_a_mutes() {
        assert_eq!(ui::mute::parse(&[0xF0, 1, b'7']), Err(UiError::Head));
        assert_eq!(
            ui::mute::parse(&[ui::bal::head(), 1, b'7']),
            Err(UiError::Head)
        );
        assert_eq!(
            ui::mute::parse(&[ui::mute::head(), 9, b'7']),
            Err(UiError::Unknown(9))
        );
    }

    /// A history arrow is which ledger, where in it, and what it is narrowed to — all three,
    /// because Discord sends the `custom_id` back and nothing else.
    #[test]
    fn a_history_arrow_survives_the_round_trip() {
        let listing = ui::history::Listing {
            screen: ui::history::Screen::Paid,
            page: 3,
            unit: Some("n".to_string()),
            discord_id: Some(100_000_000_000_000_001),
        };
        let encoded = ui::history::page_custom_id(&listing);

        assert_eq!(ui::history::parse(&parse(&encoded)), Ok(listing));
        assert!(
            encoded.chars().count() <= 100,
            "{} chars",
            encoded.chars().count()
        );
    }

    /// And an unnarrowed one, which is the same id with two empty fields.
    #[test]
    fn an_unfiltered_history_arrow_survives_it_too() {
        let listing = ui::history::Listing {
            screen: ui::history::Screen::Issued,
            page: 1,
            unit: None,
            discord_id: None,
        };

        assert_eq!(
            ui::history::parse(&parse(&ui::history::page_custom_id(&listing))),
            Ok(listing)
        );
    }

    #[test]
    fn history_arrows_preserve_colons_in_imported_units() {
        for unit in [
            "<:winvista_calcexe:870168179052253215>",
            "<a:DiamondToppogi:1133702480094580766>",
            "unit:1",
            "99999amount:",
            "pi\namount:186000",
        ] {
            for screen in [ui::history::Screen::Paid, ui::history::Screen::Issued] {
                for discord_id in [None, Some(100_000_000_000_000_001)] {
                    let listing = ui::history::Listing {
                        screen,
                        page: 2,
                        unit: Some(unit.to_owned()),
                        discord_id,
                    };
                    let encoded = ui::history::page_custom_id(&listing);
                    assert_eq!(ui::history::parse(&parse(&encoded)), Ok(listing));
                    assert!(encoded.chars().count() <= 100);
                }
            }
        }
    }

    /// Its own head, for every other space's reason: an id here carries a ledger and a filter,
    /// and `0xF0` is every other space's.
    #[test]
    fn another_spaces_head_is_not_a_histories() {
        assert_eq!(
            ui::history::parse(&[0xF0, b'p', b'a', b'i', b'd']),
            Err(UiError::Head)
        );
        assert_eq!(
            ui::history::parse(&[ui::mute::head(), b'p']),
            Err(UiError::Head)
        );
        assert_eq!(
            ui::history::parse(&[ui::history::head(), b'?']),
            Err(UiError::Head)
        );
    }
}
