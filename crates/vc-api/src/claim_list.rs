//! `VirtualCryptoWeb.Interaction.Claim.List.Options` and its helpers: the claim
//! list's filter state, packed into the thirteen bytes a `custom_id` carries.

/// `List.Options.position_t`: which side of the list is being shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    All,
    Received,
    Claimed,
}

/// `List.Options.page`: the page, or `:last` for the one the command opens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Last,
    Number(u32),
}

/// One list view's filters, as the buttons and the select menu carry them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListOptions {
    pub pending: bool,
    pub approved: bool,
    pub denied: bool,
    pub canceled: bool,
    pub position: Position,
    pub page: Page,
    /// `None` is the `0` the payload uses for "no filter".
    pub related_user: Option<u64>,
}

/// `ListOptions.length/0`: four status bits, the position, two reserved bits, a
/// 32-bit page and a 64-bit related user.
pub const LENGTH: usize = 13;

/// The 64-bit related-user field's mask, and where the fields sit in the
/// big-endian 104-bit payload.
const RELATED_USER: u32 = 0;
const PAGE: u32 = 64;
const RESERVED: u32 = 96;
const POSITION: u32 = 98;
const CANCELED: u32 = 100;
const DENIED: u32 = 101;
const APPROVED: u32 = 102;
const PENDING: u32 = 103;
const RELATED_USER_MASK: u128 = u64::MAX as u128;
const PAGE_MASK: u128 = u32::MAX as u128;

impl ListOptions {
    /// `ListOptions.encode/1`.
    pub fn encode(&self) -> Vec<u8> {
        let position = match self.position {
            Position::All => 0u128,
            Position::Received => 1,
            Position::Claimed => 2,
        };

        let page = match self.page {
            Page::Last => 0u128,
            Page::Number(page) => u128::from(page),
        };

        let value = (u128::from(self.pending) << PENDING)
            | (u128::from(self.approved) << APPROVED)
            | (u128::from(self.denied) << DENIED)
            | (u128::from(self.canceled) << CANCELED)
            | (position << POSITION)
            // The two reserved bits stay zero.
            | ((page & PAGE_MASK) << PAGE)
            | (u128::from(self.related_user.unwrap_or(0)) & RELATED_USER_MASK);

        // The payload is 104 bits, which is the low 104 bits of a 128-bit
        // big-endian number, so it starts three bytes in.
        value.to_be_bytes()[3..LENGTH + 3].to_vec()
    }

    /// `ListOptions.parse/1`: the options and whatever the caller packed after
    /// them. Elixir's pattern match has no clause for a set reserved bit or an
    /// unknown position, so those are rejected rather than ignored.
    pub fn parse(bytes: &[u8]) -> Option<(Self, &[u8])> {
        let (head, rest) = bytes.split_at_checked(LENGTH)?;

        let mut raw = [0u8; 16];
        raw[3..LENGTH + 3].copy_from_slice(head);
        let value = u128::from_be_bytes(raw);

        if (value >> RESERVED) & 0b11 != 0 {
            return None;
        }

        let position = match (value >> POSITION) & 0b11 {
            0 => Position::All,
            1 => Position::Received,
            2 => Position::Claimed,
            _ => return None,
        };

        let page = ((value >> PAGE) & PAGE_MASK) as u32;
        let related_user = ((value >> RELATED_USER) & RELATED_USER_MASK) as u64;

        Some((
            ListOptions {
                pending: (value >> PENDING) & 1 == 1,
                approved: (value >> APPROVED) & 1 == 1,
                denied: (value >> DENIED) & 1 == 1,
                canceled: (value >> CANCELED) & 1 == 1,
                position,
                page: if page == 0 {
                    Page::Last
                } else {
                    Page::Number(page)
                },
                related_user: if related_user == 0 {
                    None
                } else {
                    Some(related_user)
                },
            },
            rest,
        ))
    }
}

/// `List.Helper.encode_claim_ids/1`: how many ids follow, then each as eight
/// big-endian bytes.
pub fn encode_claim_ids(ids: &[i64]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(1 + ids.len() * 8);
    encoded.push(ids.len() as u8);

    for id in ids {
        encoded.extend_from_slice(&id.to_be_bytes());
    }

    encoded
}

/// `List.Helper.destructuring_claim_ids/1`: eight bytes per id, and a trailing
/// partial chunk is ignored, as `Stream.unfold/2` does.
pub fn destructuring_claim_ids(bytes: &[u8]) -> Vec<i64> {
    bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| i64::from_be_bytes(*chunk))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `test/virtualCrypto_web/controllers/api/interactions/claim/list/claim_list_options_test.exs`:
    /// every combination of the fields survives a round trip.
    #[test]
    fn every_list_option_round_trips() {
        for pending in [true, false] {
            for approved in [true, false] {
                for denied in [true, false] {
                    for canceled in [true, false] {
                        for position in [Position::All, Position::Received, Position::Claimed] {
                            for page in [
                                Page::Last,
                                Page::Number(1),
                                Page::Number(2),
                                Page::Number(3),
                            ] {
                                for related_user in [None, Some(123_456_789_012_345_678)] {
                                    let options = ListOptions {
                                        pending,
                                        approved,
                                        denied,
                                        canceled,
                                        position,
                                        page,
                                        related_user,
                                    };

                                    let encoded = options.encode();
                                    assert_eq!(encoded.len(), LENGTH);

                                    let (parsed, rest) =
                                        ListOptions::parse(&encoded).expect("the payload parses");

                                    assert_eq!(parsed, options);
                                    assert!(rest.is_empty(), "nothing follows the options");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_reserved_bit_is_rejected() {
        let mut encoded = ListOptions {
            pending: true,
            approved: false,
            denied: false,
            canceled: false,
            position: Position::All,
            page: Page::Last,
            related_user: None,
        }
        .encode();

        // The reserved pair is the sixth and seventh bits of the first byte.
        encoded[0] |= 0b0000_0010;

        assert_eq!(ListOptions::parse(&encoded), None);
    }

    #[test]
    fn claim_ids_round_trip() {
        let ids = [1, -1, i64::MAX];
        let encoded = encode_claim_ids(&ids);

        assert_eq!(encoded.len(), 1 + ids.len() * 8);
        assert_eq!(usize::from(encoded[0]), ids.len());

        // The count byte is the caller's to read; the ids follow it.
        assert_eq!(destructuring_claim_ids(&encoded[1..]), ids);
    }
}
