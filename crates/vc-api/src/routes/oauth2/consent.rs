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
    let mut html = String::from(
        r#"<!doctype html>
<html lang="ja"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>アプリケーションの認可 — VirtualCrypto</title>
</head><body><main><h1>アプリケーションの認可</h1>"#,
    );
    html.push_str(&format!(
        "<p><strong>{}</strong> に次のアクセスを許可します。</p><dl><dt>クライアントID</dt><dd>{}</dd><dt>サーバーID</dt><dd>{}</dd><dt>承認後の移動先</dt><dd>{}</dd></dl><h2>権限</h2><ul>",
        escape(consent.client_name.as_deref().unwrap_or(&consent.client_id)),
        escape(&consent.client_id),
        consent.guild_id,
        escape(&consent.redirect_uri),
    ));
    for scope in consent.scopes.iter().filter(|scope| !scope.is_empty()) {
        let description = if scope == "vc.issue" {
            "サーバーのプールから通貨を配布し、配布履歴を閲覧する（vc.issue）"
        } else {
            scope
        };
        html.push_str(&format!("<li>{}</li>", escape(description)));
    }
    html.push_str("</ul><h2>対象通貨</h2>");
    if consent.resources.is_empty() {
        html.push_str("<p>このサーバーのすべての通貨（今後作成する通貨を含む）</p>");
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
    html.push_str(
        r#"<button type="submit">承認する</button></form>
<p>承認しない場合は、このページを閉じてください。</p></main></body></html>"#,
    );
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
