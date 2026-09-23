//! Protect cookie-authenticated writes with browser-controlled request headers.

use axum::http::{HeaderMap, header};
use reqwest::Url;

/// Fetch Metadata distinguishes an origin from its sibling subdomains. Older
/// clients must prove the origin through Origin or Referer instead. Never derive
/// the trusted origin from Host or forwarding headers supplied with the request.
pub(super) fn same_origin(headers: &HeaderMap, site_url: &str) -> bool {
    let Ok(site) = Url::parse(site_url) else {
        return false;
    };
    if !matches!(site.scheme(), "http" | "https") || !site.has_host() {
        return false;
    }

    let source = if headers.contains_key(header::ORIGIN) {
        Some(single(headers, "origin").is_some_and(|value| {
            Url::parse(value).is_ok_and(|url| {
                url.origin() == site.origin()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.fragment().is_none()
            })
        }))
    } else if headers.contains_key(header::REFERER) {
        Some(single(headers, "referer").is_some_and(|value| {
            Url::parse(value).is_ok_and(|url| {
                url.origin() == site.origin()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
        }))
    } else {
        None
    };

    if headers.contains_key("sec-fetch-site") {
        single(headers, "sec-fetch-site") == Some("same-origin") && source != Some(false)
    } else {
        source == Some(true)
    }
}

fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "https://vc.example.com";

    #[test]
    fn browser_headers_and_legacy_fallbacks() {
        for fields in [
            vec![("sec-fetch-site", "same-origin")],
            vec![("sec-fetch-site", "same-origin"), ("origin", SITE)],
            vec![("origin", SITE)],
            vec![("origin", "https://vc.example.com:443")],
            vec![(
                "referer",
                "https://vc.example.com/oauth2/authorize?client_id=1",
            )],
        ] {
            let headers = fields
                .into_iter()
                .map(|(key, value)| (header::HeaderName::from_static(key), value.parse().unwrap()))
                .collect();
            assert!(same_origin(&headers, SITE), "{headers:?}");
        }
    }

    #[test]
    fn untrusted_or_ambiguous_requests_are_refused() {
        for fields in [
            vec![],
            vec![("origin", "null")],
            vec![("origin", "http://vc.example.com")],
            vec![("origin", "https://vc.example.com:8443")],
            vec![("origin", "https://evil.example.com")],
            vec![("origin", "https://vc.example.com.attacker.test")],
            vec![("origin", "https://vc.example.com/path")],
            vec![("origin", "https://vc.example.com@attacker.test")],
            vec![("origin", SITE), ("origin", SITE)],
            vec![("origin", "null"), ("referer", SITE)],
            vec![("sec-fetch-site", "same-site"), ("origin", SITE)],
            vec![("sec-fetch-site", "cross-site"), ("origin", SITE)],
            vec![("sec-fetch-site", "none"), ("origin", SITE)],
            vec![("sec-fetch-site", "unknown"), ("origin", SITE)],
            vec![
                ("sec-fetch-site", "same-origin"),
                ("origin", "https://evil.example.com"),
            ],
            vec![
                ("sec-fetch-site", "same-origin"),
                ("sec-fetch-site", "cross-site"),
            ],
            vec![
                ("referer", "https://evil.example.com"),
                ("host", "evil.example.com"),
            ],
        ] {
            let mut headers = HeaderMap::new();
            for (key, value) in fields {
                headers.append(header::HeaderName::from_static(key), value.parse().unwrap());
            }
            assert!(!same_origin(&headers, SITE), "{headers:?}");
        }
    }
}
