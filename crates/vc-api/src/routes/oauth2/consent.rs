//! Browser consent without JavaScript; JSON remains available to API clients.

use axum::Json;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{Html, IntoResponse, Response};

use super::Consent;

pub(super) fn respond(headers: &HeaderMap, consent: Consent) -> Response {
    let html = headers.get_all(header::ACCEPT).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value.split(',').any(|media| {
                let mut parts = media.split(';');
                parts
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/html"))
                    && !parts.any(|parameter| {
                        parameter
                            .trim()
                            .strip_prefix("q=")
                            .is_some_and(|q| q.parse::<f32>() == Ok(0.0))
                    })
            })
        })
    });
    let mut response = if html {
        Html(render(&consent)).into_response()
    } else {
        Json(consent).into_response()
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Accept"));
    response
        .headers_mut()
        .insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

fn render(consent: &Consent) -> String {
    let mut html = format!(
        r#"<!doctype html>
<html lang="{}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{} — VirtualCrypto</title>
</head><body><main><h1>{}</h1>"#,
        env!("VC_UI_LOCALE"),
        escape(message!("consent.title")),
        escape(message!("consent.title")),
    );
    // Escape the translated template and the client independently. Only our
    // own strong element is markup, even if either text contains HTML.
    let client = format!(
        "<strong>{}</strong>",
        escape(consent.client_name.as_deref().unwrap_or(&consent.client_id))
    );
    let introduction = escape(message!("consent.grantAccess")).replace("{client}", &client);
    html.push_str(&format!("<p>{introduction}</p><dl>"));
    for (label, value) in [
        (message!("consent.clientId"), consent.client_id.clone()),
        (message!("consent.serverId"), consent.guild_id.to_string()),
        (message!("consent.redirect"), consent.redirect_uri.clone()),
    ] {
        html.push_str(&format!(
            "<dt>{}</dt><dd>{}</dd>",
            escape(label),
            escape(&value)
        ));
    }
    html.push_str(&format!(
        "</dl><h2>{}</h2><ul>",
        escape(message!("consent.permissions"))
    ));
    for scope in consent.scopes.iter().filter(|scope| !scope.is_empty()) {
        let description = if scope == "vc.issue" {
            message!("consent.issueScope")
        } else {
            scope
        };
        html.push_str(&format!("<li>{}</li>", escape(description)));
    }
    html.push_str(&format!(
        "</ul><h2>{}</h2>",
        escape(message!("consent.currencies"))
    ));
    if consent.resources.is_empty() {
        html.push_str(&format!(
            "<p>{}</p>",
            escape(message!("consent.allCurrencies"))
        ));
    } else {
        html.push_str("<ul>");
        for unit in &consent.resources {
            html.push_str(&format!("<li>{}</li>", escape(unit)));
        }
        html.push_str("</ul>");
    }
    html.push_str(r#"<form method="post" action="/oauth2/authorize">"#);
    hidden(&mut html, "action", "approve");
    hidden(&mut html, "flow_id", &consent.flow_id.to_string());
    html.push_str(&format!(
        r#"<button type="submit">{}</button></form>
<p>{}</p></main></body></html>"#,
        escape(message!("consent.approve")),
        escape(message!("consent.dismiss")),
    ));
    html
}

fn hidden(html: &mut String, name: &str, value: &str) {
    html.push_str(&format!(
        r#"<input type="hidden" name="{name}" value="{}">"#,
        escape(value)
    ));
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
