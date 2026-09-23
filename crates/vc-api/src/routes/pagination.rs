//! The parts of keyset paging that every list here shares.
//!
//! `Rest.md` describes one paged list — `GET /api/v2/users/@me/claims` — and the
//! contract lists are the same idea over a different table: `limit` bounds the
//! page, `next` and `on_next` say where to resume ([`vc_core::page::Cursor`]),
//! and a page that came back exactly full carries a `link` header pointing at
//! the next one. What is *not* here is how a list rebuilds its own query string:
//! that is the list's own business, because only the list knows which of its
//! parameters the continuation has to carry.

use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::header::HOST;

use vc_core::page::Cursor;

use crate::error::ApiError;

/// How many rows a page holds when the caller does not say.
///
/// One number for every list here — the claim list, the two contract lists and the
/// statement — because a page is the list's contract with its caller rather than
/// each list's own taste. The claim list answered *every* matching claim until it
/// was given this one, which made it the only unbounded-by-default list in the
/// service; `docs/known-gaps.md` records the difference from the Elixir.
pub const PER_PAGE: i64 = 50;

/// The most rows one page may hold, however the caller asks.
///
/// [`PER_PAGE`] is the default; this is the ceiling. A limit the service has to honour is a request to read a table —
/// the first version of this took whatever number it was given, so a caller could
/// ask for a million rows with a straight face and one query.
const MAX_LIMIT: i64 = 200;

/// What a list was asked for: how many rows, and where to resume from.
///
/// The cursor is the ordered column's own value, so this is generic over it: an id for the
/// claim list and the contract family, and the merged ledger's own place for
/// `/api/v2/users/@me/transactions`, which reads two tables at once.
pub struct Page<T = i64> {
    pub limit: Option<i64>,
    pub cursor: Cursor<T>,
}

impl Page<i64> {
    /// Read `limit`, `next` and `on_next` off the query.
    pub fn asked(params: &QueryParams) -> Result<Self, ApiError> {
        let limit = limit(params)?;
        let (on_next, next) = cursors(params)?;

        let cursor = match (on_next, next) {
            (Some(value), _) => {
                Cursor::OnNext(parse_number(&value).ok_or_else(cursor_cannot_be_read)?)
            }
            (None, Some(value)) => {
                Cursor::Next(parse_number(&value).ok_or_else(cursor_cannot_be_read)?)
            }
            (None, None) => Cursor::First,
        };

        Ok(Page { limit, cursor })
    }
}

impl<T> Page<T> {
    /// The same page for a list whose cursor is not one number: `read` is the list's own reader
    /// for the value it wrote, because a cursor is the ordered column's own value and here that
    /// is not an id. A value it cannot read is `invalid_cursor`, the same complaint a
    /// non-numeric id gets, since both say the caller named a place to resume from that cannot be
    /// read.
    pub fn asked_text(
        params: &QueryParams,
        read: impl Fn(&str) -> Option<T>,
    ) -> Result<Self, ApiError> {
        let limit = limit(params)?;
        let (on_next, next) = cursors(params)?;

        let cursor = match (on_next, next) {
            (Some(value), _) => Cursor::OnNext(read(&value).ok_or_else(cursor_cannot_be_read)?),
            (None, Some(value)) => Cursor::Next(read(&value).ok_or_else(cursor_cannot_be_read)?),
            (None, None) => Cursor::First,
        };

        Ok(Page { limit, cursor })
    }

    /// The same page with a limit, for a list whose own default is not "every
    /// row": a statement written by every use has to be paged whether or not
    /// anyone asked for it.
    pub fn limited_to(mut self, default: i64) -> Self {
        if self.limit.is_none() {
            self.limit = Some(default);
        }

        self
    }

    /// The cursor to offer as `next`, when the page came back exactly full: the
    /// key of its last row. A page that is short is the end of the list, and an
    /// empty one never offers a next however small the limit.
    ///
    /// The key is the caller's, because which column orders a list is the list's
    /// business — a claim is ordered by its id, a contract by its own.
    pub fn next_cursor<R>(&self, rows: &[R], key: impl Fn(&R) -> i64) -> Option<i64> {
        let last = rows.last().map(key)?;

        (self.limit == Some(rows.len() as i64)).then_some(last)
    }

    /// The same, for a list whose cursor is not a number: the key is the row's own spelling of
    /// the ordered column, which is what a caller hands back as `next` — and what the list reads
    /// again with [`Page::asked_text`].
    pub fn next_text<R>(&self, rows: &[R], key: impl Fn(&R) -> String) -> Option<String> {
        let last = rows.last().map(key)?;

        (self.limit == Some(rows.len() as i64)).then_some(last)
    }
}

/// How many rows the caller asked for, which is read before the cursor for the claim list's
/// reason: a request that is wrong in both ways is answered with the `limit` complaint, because
/// that is the one the Elixir reaches first.
///
/// A negative limit is a client's typo, and Postgres answers it with an error of its own — a 500
/// for something the caller can fix. A limit above [`MAX_LIMIT`] is a page size this service will
/// not honour, and saying so is better than quietly answering a page and letting the caller think
/// it asked for everything. Zero is a page of nothing and is allowed. Neither is one of the
/// crashes `docs/known-gaps.md` reproduces: the non-numeric case beside them is already a 400.
fn limit(params: &QueryParams) -> Result<Option<i64>, ApiError> {
    let Some(value) = params.one("limit") else {
        return Ok(None);
    };

    let limit = parse_number(&value).ok_or(ApiError::InvalidRequest("invalid_limit"))?;

    if !(0..=MAX_LIMIT).contains(&limit) {
        return Err(ApiError::InvalidRequest("invalid_limit"));
    }

    Ok(Some(limit))
}

/// `next` and `on_next` as they arrived, whichever of the two there is: both together name two
/// places to resume from, which is a request this service refuses.
fn cursors(params: &QueryParams) -> Result<(Option<String>, Option<String>), ApiError> {
    let on_next = params.one("on_next");
    let next = params.one("next");

    if on_next.is_some() && next.is_some() {
        return Err(ApiError::InvalidRequest("invalid_cursor"));
    }

    Ok((on_next, next))
}

/// The `link` header a page offers, as `Rest.md` writes it: the scheme from
/// `x-forwarded-proto` (defaulting to `http`), the `Host` verbatim with its port,
/// the path, and the list's own query.
///
/// `None` when the header cannot be built at all, which is answered by offering
/// no continuation rather than by failing a page that was already fetched.
pub fn link(headers: &HeaderMap, path: &str, query: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(&format!(
        "<{}://{}{path}?{query}>; rel=\"next\"",
        scheme(headers),
        authority(headers)
    ))
    .ok()
}

fn scheme(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http")
        .to_string()
}

/// The `Host` header verbatim, port included: the documented `link` example is
/// `<https://localhost:4000/api/v2/users/@me/claims?...>`.
fn authority(headers: &HeaderMap) -> String {
    headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Query parameters as ordered pairs, so repeated keys survive the way Plug's
/// `statuses[]` handling preserves them.
pub struct QueryParams {
    pairs: Vec<(String, String)>,
}

impl QueryParams {
    pub fn parse(raw: &str) -> Self {
        let pairs = raw
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| match pair.split_once('=') {
                Some((key, value)) => (decode(key), decode(value)),
                None => (decode(pair), String::new()),
            })
            .collect();

        Self { pairs }
    }

    pub fn all(&self, key: &str) -> Vec<String> {
        self.pairs
            .iter()
            .filter(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
            .collect()
    }

    pub fn one(&self, key: &str) -> Option<String> {
        self.pairs
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
    }
}

/// A cursor that is not a number, which is the same complaint as two cursors at
/// once: the caller named a place to resume from, and the place cannot be read.
///
/// This used to reproduce the Elixir's crash — the value went into Ecto's
/// `bigint` cast and raised, and Phoenix turns a raise into a 500 — which
/// `docs/known-gaps.md` recorded as deliberate. It is a 400 now, for the reason a
/// negative `limit` is: a 500 for a client's typo is a status nobody can act on,
/// and the mistake is the one `invalid_cursor` already named.
pub fn cursor_cannot_be_read() -> ApiError {
    ApiError::InvalidRequest("invalid_cursor")
}

pub fn parse_number(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}

fn decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len()
                && bytes[index + 1].is_ascii_hexdigit()
                && bytes[index + 2].is_ascii_hexdigit() =>
            {
                let hex = &input[index + 1..index + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}
