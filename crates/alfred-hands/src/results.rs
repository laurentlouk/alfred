//! The organic results on a search engine's results page, in order, from agent-browser's snapshot with
//! link addresses (`snapshot -i -c -u`).
//!
//! A result's title is a link nested with a heading: Google puts the link around an `h3`, Bing and
//! DuckDuckGo put an `h2` around the link. Its real address is the link's, or the one an engine's redirect
//! carries (Bing's `/ck/a?…&u=a1<base64>`, DuckDuckGo's `/l/?uddg=`, Google's `/url?q=`). Ads go through
//! the engines' click trackers (`aclk`, `googleadservices`, `y.js?ad_domain`) and the engines' own pages
//! (Images, Maps, Help) stay on their domains, so both are left out.

use serde_json::Value;

use crate::web::parse_line;

/// One organic result: the link to click, and where it really goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultLink {
    /// The ref agent-browser needs: `@e32`.
    pub full_ref: String,
    pub name: String,
    pub target: String,
}

/// Domains of the engines themselves, and pages with nothing to read (videos).
const ENGINE_HOSTS: &[&str] = &[
    "google.",
    "gstatic.com",
    "googleusercontent.com",
    "bing.com",
    "microsoft.com",
    "msn.com",
    "duckduckgo.com",
    "duck.ai",
    "youtube.com",
];

/// How ads are marked in their addresses.
const AD_MARKS: &[&str] = &["aclk", "googleadservices", "doubleclick", "/aclick", "/y.js", "ad_domain", "ad_provider", "adurl="];

/// The organic results, in page order, each address once.
pub fn organic_results(snapshot: &Value) -> Vec<ResultLink> {
    let lines: Vec<_> = snapshot["snapshot"].as_str().unwrap_or("").lines().filter_map(parse_line).collect();
    let mut out: Vec<ResultLink> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let Some(reference) = l.reference.as_ref().filter(|_| l.role == "link" && !l.url.is_empty()) else { continue };
        let inside_heading = lines[..i].iter().rev().find(|p| p.depth < l.depth).is_some_and(|p| p.role == "heading");
        let around_heading = lines[i + 1..].iter().take_while(|c| c.depth > l.depth).any(|c| c.depth == l.depth + 1 && c.role == "heading");
        if !inside_heading && !around_heading {
            continue;
        }
        let Some(target) = real_target(&l.url) else { continue };
        if out.iter().any(|r| r.target == target) {
            continue;
        }
        out.push(ResultLink { full_ref: format!("@{reference}"), name: l.name.clone(), target });
    }
    out
}

/// Where a link really goes, or `None` for an ad, an engine's own page, or anything not http(s).
pub fn real_target(href: &str) -> Option<String> {
    if is_ad(href) {
        return None;
    }
    let target = if is_engine(&host(href)) { unwrapped(href)? } else { href.to_string() };
    let lower = target.to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) || is_ad(&target) || is_engine(&host(&target)) {
        return None;
    }
    Some(target)
}

fn is_ad(href: &str) -> bool {
    let lower = href.to_lowercase();
    AD_MARKS.iter().any(|m| lower.contains(m))
}

/// The host, lowercased, without "www."; an address without a scheme is the engine's own (relative).
fn host(href: &str) -> String {
    match href.split_once("://") {
        Some((_, rest)) => {
            let h = rest.split(['/', '?', '#']).next().unwrap_or(rest).to_lowercase();
            h.strip_prefix("www.").unwrap_or(&h).to_string()
        }
        None => "google.relative".into(),
    }
}

fn is_engine(host: &str) -> bool {
    ENGINE_HOSTS.iter().any(|e| {
        if e.ends_with('.') {
            host.starts_with(e) || host.contains(&format!(".{e}"))
        } else {
            host == *e || host.ends_with(&format!(".{e}"))
        }
    })
}

/// The address an engine's redirect carries.
fn unwrapped(href: &str) -> Option<String> {
    let query = href.split_once('?')?.1.split('#').next().unwrap_or("");
    let param = |key: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| percent_decoded(v)));
    if let Some(u) = param("u").and_then(|u| u.strip_prefix("a1").map(str::to_string)) {
        return base64url(&u).and_then(|b| String::from_utf8(b).ok());
    }
    param("uddg").or_else(|| param("q").filter(|q| q.starts_with("http")))
}

fn percent_decoded(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// URL-safe base64, padded or not.
fn base64url(s: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'-' | b'+' => Some(62),
        b'_' | b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes().take_while(|c| *c != b'=') {
        acc = (acc << 6) | u32::from(value(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redirects_are_unwrapped_and_ads_and_engine_pages_dropped() {
        // A Bing organic result, as seen live on 2026-11-24.
        let bing = "https://www.bing.com/ck/a?!&&p=4a34&ptn=3&ver=2&hsh=4&u=a1aHR0cHM6Ly93d3cuYWNjdXdlYXRoZXIuY29tL2VuL2ZyL3BhcmlzLzYyMy9ob3VybHktd2VhdGhlci1mb3JlY2FzdC82MjM&ntb=1";
        assert_eq!(real_target(bing).as_deref(), Some("https://www.accuweather.com/en/fr/paris/623/hourly-weather-forecast/623"));
        assert_eq!(
            real_target("https://duckduckgo.com/l/?uddg=https%3A%2F%2Fmeteofrance.com%2Fparis&rut=abc").as_deref(),
            Some("https://meteofrance.com/paris")
        );
        assert_eq!(
            real_target("https://www.google.com/url?q=https://www.bbc.com/weather&sa=U").as_deref(),
            Some("https://www.bbc.com/weather")
        );
        assert_eq!(real_target("https://meteofrance.com/previsions").as_deref(), Some("https://meteofrance.com/previsions"));
        for no in [
            "https://www.googleadservices.com/pagead/aclk?sa=L&adurl=https://shop.example",
            "https://www.google.com/aclk?sa=l&ai=x",
            "https://www.bing.com/aclick?ld=x&u=aHR0cHM6Ly9zaG9w",
            "https://duckduckgo.com/y.js?ad_domain=shop.example",
            "https://www.bing.com/ck/a?!&&p=1&u=a1L2ltYWdlcy9zZWFyY2g", // Bing's own Images tab
            "https://www.google.com/search?q=paris&tbm=isch",
            "https://www.youtube.com/watch?v=x",
            "javascript:void(0)",
        ] {
            assert_eq!(real_target(no), None, "{no}");
        }
    }

    #[test]
    fn the_top_organic_result_is_a_titled_link_off_the_engine() {
        let lines = [
            "- navigation \"Filters\" [ref=e1]",
            "  - link \"Images\" [ref=e2, url=https://www.bing.com/images/search?q=weather]",
            "- heading \"Sponsored: Weather Pro\" [level=2, ref=e3]",
            "  - link \"Weather Pro\" [ref=e4, url=https://www.bing.com/aclick?ld=e8&u=aHR0cHM6Ly9hZHM]",
            "- link \"accuweather.com\" [ref=e5, url=https://www.accuweather.com/en/fr/paris]",
            "- heading \"Paris Weather | AccuWeather\" [level=2, ref=e6]",
            "  - link \"Paris Weather | AccuWeather\" [ref=e7, url=https://www.accuweather.com/en/fr/paris]",
            "- link \"Météo Paris - Météo-France\" [ref=e8, url=https://meteofrance.com/paris]",
            "  - heading \"Météo Paris - Météo-France\" [level=3, ref=e9]",
            "- heading \"Météo Paris again\" [level=3, ref=e10]",
            "  - link \"Météo Paris again\" [ref=e11, url=https://meteofrance.com/paris]",
        ];
        let snap = json!({ "snapshot": lines.join("\n") });
        let found = organic_results(&snap);
        assert_eq!(
            found.iter().map(|r| (r.full_ref.as_str(), r.target.as_str())).collect::<Vec<_>>(),
            [("@e7", "https://www.accuweather.com/en/fr/paris"), ("@e8", "https://meteofrance.com/paris")],
            "the ad, the engine's tab and the bare site link are skipped; each address once"
        );
    }
}
