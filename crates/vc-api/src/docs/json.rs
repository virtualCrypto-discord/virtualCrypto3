//! The document `GET /api/docs` serves, which the site renders.
//!
//! The SPA is given the same content the Discord screens are, taken apart far
//! enough that it has nothing to parse: prose arrives as spans with the marks
//! already off it, so the page interpolates text and escapes nothing, and the
//! three inline marks are understood in one place — here — rather than in Rust
//! for Discord and in TypeScript for the browser.

use serde_json::{Value, json};

use super::api::Endpoint;
use super::{Block, Listing, OptionLine, Page, Section, Showing, api, pages, resolve, showings};
use crate::state::Links;

/// Everything: the pages, the commands, the endpoints.
pub fn document(links: &Links) -> Value {
    json!({
        "pages": pages::all()
            .iter()
            .map(|page| page_json(page, links))
            .collect::<Vec<_>>(),
        "commands": showings()
            .iter()
            .map(|showing| command_json(showing, links))
            .collect::<Vec<_>>(),
        "endpoints": api::all()
            .iter()
            .map(|endpoint| endpoint_json(endpoint, links))
            .collect::<Vec<_>>(),
    })
}

/// Every field that a person reads as prose travels as spans, and the few that
/// do not are the ones where a mark would be wrong: `summary` is a tooltip,
/// `usage` is a fence, and a fence is where a backtick is a backtick.
fn page_json(page: &Page, links: &Links) -> Value {
    json!({
        "slug": page.slug,
        "title": page.title,
        "summary": resolve(page.summary, links),
        "listing": listing_json(page.listing),
        "sections": sections_json(page.sections, links),
    })
}

/// What the page shows under its prose, as a word rather than a flag: the page
/// about commands shows the command list, and the page about the API shows the
/// endpoints. `null` is a page that is prose.
fn listing_json(listing: Listing) -> Value {
    match listing {
        Listing::None => Value::Null,
        Listing::Commands => json!("commands"),
        Listing::Endpoints => json!("endpoints"),
    }
}

fn command_json(showing: &Showing, links: &Links) -> Value {
    json!({
        "name": showing.name,
        "description": spans_json(&showing.description, links),
        "admin_only": showing.admin_only,
        "usage": showing.usage.iter().map(|line| resolve(line, links)).collect::<Vec<_>>(),
        "options": showing
            .options
            .iter()
            .map(|option| option_json(option, links))
            .collect::<Vec<_>>(),
        "sections": sections_json(showing.sections, links),
    })
}

fn option_json(option: &OptionLine, links: &Links) -> Value {
    json!({
        "name": option.name,
        "description": spans_json(&option.description, links),
        "required": option.required,
        "kind": kind_json(option),
        "kind_label": option.kind.label(),
        "autocomplete": option.autocomplete,
        "children": option
            .children
            .iter()
            .map(|child| option_json(child, links))
            .collect::<Vec<_>>(),
    })
}

/// The option's type as a word a script can switch on, beside the Japanese
/// [`OptionLine::kind`] label the page shows.
fn kind_json(option: &OptionLine) -> &'static str {
    match option.kind {
        super::OptionKind::Subcommand => "subcommand",
        super::OptionKind::Text => "text",
        super::OptionKind::Integer => "integer",
        super::OptionKind::Boolean => "boolean",
        super::OptionKind::User => "user",
    }
}

fn endpoint_json(endpoint: &Endpoint, links: &Links) -> Value {
    json!({
        "method": endpoint.method,
        "path": endpoint.path,
        "summary": spans_json(endpoint.summary, links),
        "access": spans_json(endpoint.access, links),
        "fields": endpoint
            .fields
            .iter()
            .map(|field| spans_json(field, links))
            .collect::<Vec<_>>(),
        // A fence, so the lines travel as they are: a mark in a fence is a mark people
        // would see.
        "example": endpoint.example.as_ref().map(|example| json!({
            "request": example.request,
            "response": example.response,
        })),
        "notes": endpoint
            .notes
            .iter()
            .map(|note| spans_json(note, links))
            .collect::<Vec<_>>(),
        "errors": endpoint
            .errors
            .iter()
            .map(|line| spans_json(line, links))
            .collect::<Vec<_>>(),
    })
}

fn sections_json(sections: &'static [Section], links: &Links) -> Vec<Value> {
    sections
        .iter()
        .map(|section| {
            json!({
                "heading": section.heading,
                "blocks": section.blocks.iter().map(|block| block_json(block, links)).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn block_json(block: &Block, links: &Links) -> Value {
    match block {
        Block::Text(text) => json!({ "kind": "text", "spans": spans_json(text, links) }),
        Block::List(items) => json!({
            "kind": "list",
            "items": items
                .iter()
                .map(|item| spans_json(item, links))
                .collect::<Vec<_>>(),
        }),
        Block::Code(lines) => json!({ "kind": "code", "lines": lines }),
    }
}

fn spans_json(text: &str, links: &Links) -> Vec<Value> {
    spans(&resolve(text, links))
        .into_iter()
        .map(|span| {
            json!({
                "text": span.text,
                "bold": span.bold,
                "code": span.code,
                "href": span.href,
            })
        })
        .collect()
}

/// One run of prose with the marks taken off it.
#[derive(Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub code: bool,
    pub href: Option<String>,
}

impl Span {
    fn plain(text: &str) -> Self {
        Span {
            text: text.to_owned(),
            bold: false,
            code: false,
            href: None,
        }
    }
}

/// `**bold**`, `` `code` `` and `[label](url)`, taken apart.
///
/// Total, and deliberately: a mark with no close is prose, because the
/// alternative is text that a person wrote and the page does not show. Only
/// bold nests — its inside is read again, so code and links keep their marks —
/// while a label and a code span are text, not more prose.
pub fn spans(text: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut rest = text;

    while !rest.is_empty() {
        let bold = rest.find("**");
        let code = rest.find('`');
        let link = rest.find('[');

        let Some(at) = [bold, code, link].into_iter().flatten().min() else {
            push(&mut out, Span::plain(rest));
            break;
        };

        if at > 0 {
            push(&mut out, Span::plain(&rest[..at]));
        }

        let tail = &rest[at..];

        // The mark that starts here, if it closes.
        // Bold is the one mark whose inside is still prose: a heading like
        // `**`/create` で…**` keeps its code, and each run inside is bold too.
        if bold == Some(at)
            && let Some(close) = tail[2..].find("**")
        {
            for inner in spans(&tail[2..2 + close]) {
                push(&mut out, Span { bold: true, ..inner });
            }
            rest = &tail[2 + close + 2..];
            continue;
        }

        if code == Some(at)
            && let Some(close) = tail[1..].find('`')
        {
            push(
                &mut out,
                Span {
                    text: tail[1..1 + close].to_owned(),
                    code: true,
                    ..Span::plain("")
                },
            );
            rest = &tail[1 + close + 1..];
            continue;
        }

        if link == Some(at)
            && let Some((label, href, after)) = link_at(tail)
        {
            push(
                &mut out,
                Span {
                    text: label.to_owned(),
                    href: Some(href.to_owned()),
                    ..Span::plain("")
                },
            );
            rest = after;
            continue;
        }

        // A mark that does not close is one character of prose — two, for the
        // one that is two — and the scan goes on after it.
        let width = if bold == Some(at) { 2 } else { 1 };

        push(&mut out, Span::plain(&rest[..at + width]));
        rest = &rest[at + width..];
    }

    out
}

/// `[label](url)`, or `None` when the two halves are not both there. An empty
/// address is not a link: `[label]()` is what a half-typed one leaves behind.
fn link_at(tail: &str) -> Option<(&str, &str, &str)> {
    let after_bracket = tail.strip_prefix('[')?;
    let close = after_bracket.find(']')?;
    let label = &after_bracket[..close];

    let rest = after_bracket[close + 1..].strip_prefix('(')?;
    let end = rest.find(')')?;

    if end == 0 {
        return None;
    }

    Some((label, &rest[..end], &rest[end + 1..]))
}

/// Adjacent runs of unmarked text are one span: nothing reads two spans where
/// one says the same thing, and a mark that turns out to be literal must not
/// split the sentence it sits in.
fn push(out: &mut Vec<Span>, span: Span) {
    if let Some(last) = out.last_mut()
        && unmarked(last)
        && unmarked(&span)
    {
        last.text.push_str(&span.text);
        return;
    }

    out.push(span);
}

fn unmarked(span: &Span) -> bool {
    !span.bold && !span.code && span.href.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(text: &str) -> Vec<Span> {
        spans(text)
    }

    #[test]
    fn plain_prose_is_one_span() {
        assert_eq!(
            marked("そのままの文です。"),
            vec![Span::plain("そのままの文です。")]
        );
    }

    #[test]
    fn the_three_marks_are_taken_apart() {
        assert_eq!(
            marked("**強調**と`コード`と[ラベル](https://example.test/)です"),
            vec![
                Span {
                    text: "強調".to_owned(),
                    bold: true,
                    ..Span::plain("")
                },
                Span::plain("と"),
                Span {
                    text: "コード".to_owned(),
                    code: true,
                    ..Span::plain("")
                },
                Span::plain("と"),
                Span {
                    text: "ラベル".to_owned(),
                    href: Some("https://example.test/".to_owned()),
                    ..Span::plain("")
                },
                Span::plain("です"),
            ]
        );
    }

    /// A usage line carries `< >`, which is why nothing here treats one as a
    /// mark: `/pay amount:<枚数>` is prose from end to end.
    #[test]
    fn an_angle_bracket_is_prose() {
        assert_eq!(
            marked("/pay unit:<通貨の単位>"),
            vec![Span::plain("/pay unit:<通貨の単位>")]
        );
    }

    /// A mark that does not close is a character of prose rather than text that
    /// vanishes: these are pages people wrote by hand.
    #[test]
    fn an_unclosed_mark_is_literal() {
        assert_eq!(marked("**閉じない"), vec![Span::plain("**閉じない")]);
        assert_eq!(marked("`閉じない"), vec![Span::plain("`閉じない")]);
        assert_eq!(marked("[閉じない"), vec![Span::plain("[閉じない")]);
        assert_eq!(marked("[ラベル]()"), vec![Span::plain("[ラベル]()")]);

        // Whatever follows the mark is still read: the second half of the line
        // is not lost to the first half's mistake.
        assert_eq!(
            marked("**閉じない と`コード`"),
            vec![
                Span::plain("**閉じない と"),
                Span {
                    text: "コード".to_owned(),
                    code: true,
                    ..Span::plain("")
                }
            ]
        );
    }

    /// The FAQ's headings are bold and name a command: the code inside keeps
    /// its mark rather than showing its backticks.
    #[test]
    fn marks_inside_bold_are_read() {
        assert_eq!(
            marked("**`/create` で失敗する** 説明"),
            vec![
                Span {
                    text: "/create".to_owned(),
                    bold: true,
                    code: true,
                    href: None,
                },
                Span {
                    text: " で失敗する".to_owned(),
                    bold: true,
                    ..Span::plain("")
                },
                Span::plain(" 説明"),
            ]
        );
    }

    #[test]
    fn an_empty_string_has_no_spans() {
        assert_eq!(marked(""), Vec::<Span>::new());
    }
}
