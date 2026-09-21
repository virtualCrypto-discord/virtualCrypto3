//! The documentation: one source, two renderings, and the joins that hold them
//! together.
//!
//! What these lock is not the prose — that is edited freely — but the
//! connections: a command that is registered and has nothing written about it, an
//! endpoint the reference has invented, an address nothing fills in. Each one is
//! a page that lies to somebody, and each one is invisible in a diff.

mod support;

use std::path::PathBuf;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use serde_json::Value;
use sqlx::PgPool;
use support::{fake, links, state};
use tower::ServiceExt;

use vc_api::docs::{self, Block, Listing, Section, api};

/// The routes' own source, which is what an endpoint in the reference has to be
/// found in. Read as text rather than as a router, because axum has no way to ask
/// what it was given — and the literal a path is written as is the whole of what
/// has to be true.
const ROUTER_SOURCES: [&str; 9] = [
    include_str!("../src/routes/mod.rs"),
    include_str!("../src/routes/v2/mod.rs"),
    include_str!("../src/routes/oauth2.rs"),
    include_str!("../src/routes/oauth2_token.rs"),
    include_str!("../src/routes/oauth2_clients.rs"),
    include_str!("../src/routes/connect.rs"),
    include_str!("../src/routes/grants.rs"),
    include_str!("../src/routes/grant_requests.rs"),
    include_str!("../src/routes/web.rs"),
];

/// Every command is registered, and every registered command is written about.
///
/// An example is what a caller copies: a JSON answer that is not JSON is worse than none, and
/// the body it shows is the body the tests assert.
#[test]
fn every_example_answer_is_json_or_nothing() {
    for endpoint in api::all() {
        let Some(example) = &endpoint.example else {
            continue;
        };

        let body = example.response.join("\n");

        if body.starts_with('{') || body.starts_with('[') {
            serde_json::from_str::<serde_json::Value>(&body).unwrap_or_else(|error| {
                panic!("{} {}: {error}\n{body}", endpoint.method, endpoint.path)
            });
        }
    }
}

/// Every endpoint says what it refuses: a reference that lists what a call does
/// and not when it is answered with a failure is one a caller learns the rest of
/// from a request that went wrong.
///
/// And each line opens with the status, because that is the thing a caller scans
/// for — a list of conditions with the numbers buried in it is a list nobody
/// reads twice.
#[test]
fn every_endpoint_says_what_it_refuses() {
    for endpoint in api::all() {
        assert!(
            !endpoint.errors.is_empty(),
            "{} {} has no errors written down",
            endpoint.method,
            endpoint.path
        );

        for line in endpoint.errors {
            let status: String = line.chars().take(3).collect();

            assert!(
                status.len() == 3 && status.chars().all(|character| character.is_ascii_digit()),
                "{} {}: {line} does not open with a status",
                endpoint.method,
                endpoint.path
            );
        }
    }
}

/// The two lists are one list: Discord's picker, the bot's `/help` and the site's
/// command page are the same commands, and a command that is only in one of them
/// is a command nobody can find out about — or a page about something that does
/// not exist.
#[test]
fn a_registered_command_is_written_about_and_nothing_else_is() {
    let registered: Vec<String> = vc_api::discord_commands::commands()
        .iter()
        .filter_map(|command| command["name"].as_str().map(str::to_owned))
        .collect();

    let written: Vec<&str> = docs::commands::all()
        .iter()
        .map(|entry| entry.name)
        .collect();

    assert_eq!(
        written,
        registered.iter().map(String::as_str).collect::<Vec<_>>(),
        "the prose is one entry per command, in registration order"
    );
}

/// What both renderings open with is Discord's own sentence, and whether the
/// administrator bit is asked for is Discord's own field. Written down anywhere
/// else, the screens and Discord's picker could come to disagree.
#[test]
fn a_command_is_described_the_way_discord_describes_it() {
    for showing in docs::showings() {
        let registered = vc_api::discord_commands::commands()
            .into_iter()
            .find(|command| command["name"].as_str() == Some(showing.name.as_str()))
            .expect("a registered command");

        assert_eq!(
            showing.description,
            registered["description"].as_str().expect("a description"),
            "{}",
            showing.name
        );
        assert_eq!(
            showing.admin_only,
            registered
                .get("default_member_permissions")
                .and_then(Value::as_str)
                == Some("0"),
            "{}",
            showing.name
        );
    }
}

/// A command with nothing written about it would render as a name and a
/// sentence: the shape is registered and the prose is the part a person needs.
#[test]
fn every_command_says_how_it_is_typed_and_why() {
    for showing in docs::showings() {
        assert!(!showing.usage.is_empty(), "{} has no usage", showing.name);
        assert!(
            !showing.sections.is_empty(),
            "{} has no prose",
            showing.name
        );
        assert!(
            !showing.description.is_empty(),
            "{} has no description",
            showing.name
        );
        assert!(
            showing.description.chars().count() <= 100,
            "{}: Discord caps a description at 100 characters",
            showing.name
        );

        let slash = format!("/{}", showing.name);
        for line in showing.usage {
            assert!(
                line.starts_with(&slash),
                "{}: {line} is not a line of {slash}",
                showing.name
            );
        }
    }
}

/// Every path the reference lists is a path the router has.
#[test]
fn every_documented_endpoint_is_a_route() {
    for endpoint in api::all() {
        let quoted = format!("\"{}\"", endpoint.path);

        assert!(
            ROUTER_SOURCES.iter().any(|source| source.contains(&quoted)),
            "{} {} is not a route any more",
            endpoint.method,
            endpoint.path
        );
    }
}

/// And every route the v2 API registers is in the reference: a route nobody
/// wrote down is a route nobody can call.
#[test]
fn every_v2_route_is_in_the_reference() {
    for path in quoted_paths(include_str!("../src/routes/v2/mod.rs"), "/api/") {
        assert!(
            api::all().iter().any(|endpoint| endpoint.path == path),
            "{path} is registered and not in the reference"
        );
    }
}

/// The addresses are the deployment's, so the prose writes them as `{site}` and
/// friends. A fourth one would be a hole in a published page.
#[test]
fn the_prose_writes_only_the_addresses_that_are_filled_in() {
    for text in prose() {
        for name in braces(text) {
            let address = format!("{{{name}}}");

            assert!(
                docs::PLACEHOLDERS.contains(&address.as_str()),
                "{address} is not one of {:?}: {text}",
                docs::PLACEHOLDERS
            );
        }
    }

    // And with the addresses in, nothing of that shape is left.
    for text in prose() {
        let rendered = docs::resolve(text, &links());

        assert!(
            braces(&rendered).is_empty(),
            "something was left unfilled: {rendered}"
        );
    }
}

/// A heading with nothing under it, or a bullet that is empty, is a page that
/// looks finished and is not.
#[test]
fn nothing_written_is_empty() {
    for page in docs::pages::all() {
        assert!(!page.title.is_empty(), "{}", page.slug);
        assert!(!page.summary.is_empty(), "{}", page.slug);
        assert!(!page.sections.is_empty(), "{}", page.slug);

        sections_are_full(page.sections);
    }

    for command in docs::commands::all() {
        sections_are_full(command.sections);
    }

    for endpoint in api::all() {
        assert!(!endpoint.summary.is_empty(), "{}", endpoint.path);
        assert!(!endpoint.access.is_empty(), "{}", endpoint.path);
    }

    // The two lists have to be shown by a page, or the document is a document
    // nobody draws the commands from.
    assert!(
        docs::pages::all()
            .iter()
            .any(|page| page.listing == Listing::Commands)
    );
    assert!(
        docs::pages::all()
            .iter()
            .any(|page| page.listing == Listing::Endpoints)
    );

    let mut slugs: Vec<&str> = docs::pages::all().iter().map(|page| page.slug).collect();
    let written = slugs.len();
    slugs.sort_unstable();
    slugs.dedup();

    assert_eq!(written, slugs.len(), "two pages share a slug");
}

/// The one read the site makes: everything, with this deployment's addresses.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_site_reads_one_document(pool: PgPool) {
    let (status, body) = fetch(vc_api::router(state(pool, fake())), "/api/docs", None).await;

    assert_eq!(status, 200, "body: {body}");

    let document: Value = serde_json::from_str(&body).expect("the document is json");

    for key in ["pages", "commands", "endpoints"] {
        assert!(document[key].is_array(), "{key}: {body}");
    }

    assert_eq!(
        document["pages"].as_array().expect("pages").len(),
        docs::pages::all().len()
    );
    assert_eq!(
        document["commands"].as_array().expect("commands").len(),
        vc_api::discord_commands::commands().len()
    );
    assert_eq!(
        document["endpoints"].as_array().expect("endpoints").len(),
        api::all().len()
    );

    // The errors are the site's too: a list that never reached the page would be
    // a list nobody reads.
    assert!(
        document["endpoints"]
            .as_array()
            .expect("endpoints")
            .iter()
            .all(|endpoint| endpoint["errors"]
                .as_array()
                .is_some_and(|errors| !errors.is_empty())),
        "an endpoint's errors did not reach the document"
    );

    assert!(!body.contains("{site}"), "an address was left unfilled");
    assert!(body.contains(&links().site_url), "the deployment's address");
}

/// The options the site shows are the ones Discord was told about, which is what
/// makes the page and the picker one answer rather than two.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_options_are_the_registered_ones(pool: PgPool) {
    let (_, body) = fetch(vc_api::router(state(pool, fake())), "/api/docs", None).await;
    let document: Value = serde_json::from_str(&body).expect("the document is json");

    let commands = document["commands"].as_array().expect("commands");
    let pay = commands
        .iter()
        .find(|command| command["name"] == "pay")
        .expect("pay is documented");

    let names: Vec<&str> = pay["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|option| option["name"].as_str().expect("a name"))
        .collect();

    assert_eq!(names, ["unit", "user", "amount"]);
    assert_eq!(pay["options"][0]["autocomplete"], true, "{pay}");
    assert_eq!(pay["options"][1]["kind"], "user", "{pay}");
    assert_eq!(pay["options"][1]["kind_label"], "ユーザー", "{pay}");
    assert_eq!(pay["options"][2]["required"], true, "{pay}");

    // A subcommand's own options are under it, which is the shape `claim` needs.
    let claim = commands
        .iter()
        .find(|command| command["name"] == "claim")
        .expect("claim is documented");
    let approve = claim["options"]
        .as_array()
        .expect("options")
        .iter()
        .find(|option| option["name"] == "approve")
        .expect("approve");

    assert_eq!(approve["kind"], "subcommand", "{approve}");
    assert_eq!(approve["children"][0]["name"], "id", "{approve}");
}

/// The scope the document is read through is the API's, so a caller that cannot
/// accept JSON is refused before the handler runs.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_document_needs_an_accept_json_satisfies(pool: PgPool) {
    let app = || vc_api::router(state(pool.clone(), fake()));

    let (status, _) = fetch(app(), "/api/docs", Some("text/html")).await;
    assert_eq!(status, 406);

    let (status, _) = fetch(app(), "/api/docs", Some("application/json")).await;
    assert_eq!(status, 200);
}

/// `/document/*` is the SPA's, not the server's: a link the bot sends to
/// `/document/commands` has to arrive at the page that draws it. A route added
/// here would answer it with something else, so this is the test to fail while
/// that is being decided.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_documentation_paths_stay_the_spas(pool: PgPool) {
    let (status, body) = fetch(
        vc_api::router_with_web(state(pool, fake()), built_spa()),
        "/document/commands",
        None,
    )
    .await;

    assert_eq!(status, 200, "body: {body}");
    assert!(body.contains("<title>spa</title>"), "body: {body}");
}

/// A directory holding a built SPA, which is all `ServeDir` needs of one.
fn built_spa() -> PathBuf {
    let root = std::env::temp_dir().join(format!("vc-docs-{}", std::process::id()));
    std::fs::create_dir_all(root.join("assets")).expect("the fixture directory");
    std::fs::write(root.join("index.html"), "<!doctype html><title>spa</title>").expect("index");

    root
}

async fn fetch(app: Router, uri: &str, accept: Option<&str>) -> (u16, String) {
    let mut request = Request::builder().uri(uri);

    if let Some(accept) = accept {
        request = request.header("accept", accept);
    }

    let response = app
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("router response");

    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");

    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Every piece of prose the documentation writes: the sections of every page and
/// every command, and each endpoint's own sentences.
///
/// Not the paths, the names or the titles — a path has `{id}` in it and is a
/// route, which is what the router checks are for. Not the fences either: a fence
/// is literal, so an address written inside one would be a code sample rather
/// than a link, and that is on purpose.
fn prose() -> Vec<&'static str> {
    let mut found = Vec::new();

    for command in docs::commands::all() {
        collect(command.sections, &mut found);
    }

    for page in docs::pages::all() {
        collect(page.sections, &mut found);
    }

    for endpoint in api::all() {
        found.push(endpoint.summary);
        found.push(endpoint.access);
        found.extend(endpoint.notes.iter().copied());
        found.extend(endpoint.errors.iter().copied());
    }

    found
}

fn collect(sections: &'static [Section], found: &mut Vec<&'static str>) {
    for section in sections {
        for block in section.blocks {
            match block {
                Block::Text(text) => found.push(text),
                Block::List(items) => found.extend(items.iter().copied()),
                Block::Code(_) => {}
            }
        }
    }
}

/// The `{word}`s in a text: the shape an address is written in, and the shape a
/// mistake is. The error bodies the API page quotes are `{"error": ...}`, which
/// this does not mistake for one.
fn braces(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;

    while let Some(at) = rest.find('{') {
        let after = &rest[at + 1..];

        let Some(end) = after.find('}') else {
            break;
        };

        let inside = &after[..end];

        if !inside.is_empty()
            && inside
                .chars()
                .all(|character| character.is_ascii_lowercase() || character == '_')
        {
            found.push(inside);
        }

        rest = &after[end..];
    }

    found
}

/// The quoted literals in a source that begin with `prefix`.
fn quoted_paths(source: &str, prefix: &str) -> Vec<String> {
    let needle = format!("\"{prefix}");
    let mut found = Vec::new();
    let mut rest = source;

    while let Some(at) = rest.find(&needle) {
        // Past the opening quote.
        rest = &rest[at + 1..];

        let Some(end) = rest.find('"') else {
            break;
        };

        found.push(rest[..end].to_owned());
        rest = &rest[end..];
    }

    found
}

fn sections_are_full(sections: &'static [Section]) {
    for section in sections {
        assert!(
            !section.blocks.is_empty(),
            "{:?} has nothing under it",
            section.heading
        );

        for block in section.blocks {
            match block {
                Block::Text(text) => assert!(!text.trim().is_empty()),
                Block::List(items) => {
                    assert!(!items.is_empty(), "{:?}", section.heading);

                    for item in *items {
                        assert!(!item.trim().is_empty(), "{:?}", section.heading);
                    }
                }
                Block::Code(lines) => {
                    assert!(!lines.is_empty(), "{:?}", section.heading);

                    for line in *lines {
                        assert!(!line.is_empty(), "{:?}", section.heading);
                    }
                }
            }
        }
    }
}
