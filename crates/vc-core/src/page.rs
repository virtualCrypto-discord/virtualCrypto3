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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    First,
    Next(i64),
    OnNext(i64),
}

impl Cursor {
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Cursor::First => "first",
            Cursor::Next(_) => "next",
            Cursor::OnNext(_) => "on_next",
        }
    }

    pub(crate) fn value(self) -> Option<i64> {
        match self {
            Cursor::First => None,
            Cursor::Next(value) | Cursor::OnNext(value) => Some(value),
        }
    }
}
