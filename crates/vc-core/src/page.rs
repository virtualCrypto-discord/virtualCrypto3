//! Where a page of a list resumes from.
//!
//! One shape serves every keyset-paged list this service answers. The claim list
//! was the first and is still the one `Rest.md` describes: the cursor is the
//! ordered column's value itself — an id — rather than an opaque token, and the
//! two directions are named the way the specification names them, `next` for
//! "after this row, this row excluded" and `on_next` for "from this row on".
//!
//! Nothing here knows what it is paging: the query is the caller's, and what it
//! takes from this is which comparison to make and against what.

/// Where to resume from. `First` is the start, `Next` is exclusive (`<`/`>`) and
/// `OnNext` is inclusive (`<=`/`>=`).
///
/// The value is the ordered column's own, which is why this is not fixed to a number: an id
/// for most lists here, and the merged ledger's own place for the one that reads two tables.
/// A cursor is the list's to spell, and what a list orders by is the list's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor<T = i64> {
    First,
    Next(T),
    OnNext(T),
}

impl<T> Cursor<T> {
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Cursor::First => "first",
            Cursor::Next(_) => "next",
            Cursor::OnNext(_) => "on_next",
        }
    }

    pub(crate) fn value(self) -> Option<T> {
        match self {
            Cursor::First => None,
            Cursor::Next(value) | Cursor::OnNext(value) => Some(value),
        }
    }
}
