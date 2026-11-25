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

/// The videos on YouTube's results page, in order: each `watch?v=` link (never a Short, a channel or an
/// ad) with its title (without the length YouTube reads out after it) and the channel linked after it.
pub fn youtube_videos(snapshot: &Value) -> Vec<alfred_core::FoundVideo> {
    let lines: Vec<_> = snapshot["snapshot"].as_str().unwrap_or("").lines().filter_map(parse_line).collect();
    let is_video = |l: &crate::web::Line| l.role == "link" && video_id(&l.url).is_some();
    let mut out: Vec<alfred_core::FoundVideo> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let Some(id) = (is_video(l)).then(|| video_id(&l.url)).flatten() else { continue };
        // The result is the container its heading sits in, up to the next one.
        let start = lines[..i].iter().rposition(|p| p.depth + 1 < l.depth.max(1)).unwrap_or(i);
        let next = |n: &crate::web::Line| if start < i { n.depth <= lines[start].depth } else { is_video(n) || n.url.contains("/shorts/") };
        let block_end = lines[i + 1..].iter().position(next).map_or(lines.len(), |p| i + 1 + p);
        if lines[start..block_end].iter().any(|b| is_ad_label(&b.name)) {
            continue;
        }
        let url = format!("https://www.youtube.com/watch?v={id}");
        if out.iter().any(|v| v.url == url) {
            continue;
        }
        let channel = lines[i + 1..block_end]
            .iter()
            .find(|n| n.role == "link" && (n.url.contains("youtube.com/@") || n.url.contains("youtube.com/channel/")))
            .map(|n| channel_name(&n.name))
            .unwrap_or_default();
        let title = without_length(l.name.trim());
        if title.is_empty() {
            continue;
        }
        out.push(alfred_core::FoundVideo { title, channel, url });
    }
    out
}

/// "Sponsored", "Sponsorisé · Acme": how YouTube marks an ad among the results.
fn is_ad_label(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    ["sponsored", "sponsorisé", "sponsorisée", "annonce", "ad", "promoted"]
        .iter()
        .any(|m| n == *m || n.strip_prefix(m).is_some_and(|rest| rest.starts_with([' ', '·', '•', ':'])))
}

/// A channel's name without the words its avatar link adds: "Accéder à la chaîne 3Blue1Brown" →
/// "3Blue1Brown".
fn channel_name(link: &str) -> String {
    const PREFIXES: &[&str] =
        &["Go to channel ", "Accéder à la chaîne ", "Zum Kanal ", "Ir al canal ", "Vai al canale ", "Ir para o canal "];
    let name = link.trim();
    PREFIXES.iter().find_map(|p| name.strip_prefix(p)).unwrap_or(name).trim().to_string()
}

/// A watch link's video id: `https://www.youtube.com/watch?v=wjZofJX0v4M&pp=…` → `wjZofJX0v4M`.
fn video_id(href: &str) -> Option<String> {
    let lower = href.to_lowercase();
    if !(lower.starts_with("https://www.youtube.com/watch?") || lower.starts_with("https://youtube.com/watch?")) || is_ad(href) {
        return None;
    }
    let query = href.split_once('?')?.1;
    let id = query.split('&').find_map(|kv| kv.strip_prefix("v="))?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then(|| id.to_string())
}

/// A title without the length YouTube appends to a link's name: "Transformers explained 6 minutes et 30
/// secondes" → "Transformers explained".
fn without_length(title: &str) -> String {
    const UNITS: &[&str] = &[
        "second", "seconds", "seconde", "secondes", "minute", "minutes", "hour", "hours", "heure", "heures", "sekunden", "minuten",
        "stunde", "stunden", "segundos", "minutos", "horas",
    ];
    const JOINS: &[&str] = &["et", "and", "und", "y", ","];
    let mut words: Vec<&str> = title.split_whitespace().collect();
    loop {
        let last = |w: &[&str], back: usize| w.len().checked_sub(back + 1).map(|i| w[i].trim_end_matches(',').to_lowercase());
        match (last(&words, 0), last(&words, 1)) {
            (Some(unit), Some(n)) if UNITS.contains(&unit.as_str()) && n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => {
                words.truncate(words.len() - 2);
                while words.last().is_some_and(|w| JOINS.contains(&w.to_lowercase().as_str())) {
                    words.pop();
                }
                if let Some(w) = words.last_mut() {
                    *w = w.trim_end_matches(',');
                }
            }
            _ => break,
        }
    }
    words.join(" ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// YouTube's results as `snapshot -i -c -u` gave them on 2026-11-25 (in French), cut down.
    const YOUTUBE: &str = r#"- link "Shorts" [ref=e9, url=https://www.youtube.com/shorts/]
- generic "6:30Comment fonctionne un transformateur" [ref=e22] clickable [cursor:pointer]
  - heading "Comment fonctionne un transformateur – Principe de fonctionnement en génie électrique 6 minutes et 30 secondes" [level=3, ref=e64]
    - link "Comment fonctionne un transformateur – Principe de fonctionnement en génie électrique 6 minutes et 30 secondes" [ref=e115, url=https://www.youtube.com/watch?v=UchitHGF4n8&pp=ygUYaG93IGRv]
  - link "Accéder à la chaîne The Engineering Mindset" [ref=e226, url=https://www.youtube.com/@EngineeringMindset]
  - link "The Engineering Mindset" [ref=e176, url=https://www.youtube.com/@EngineeringMindset]
  - button "Autres actions" [ref=e116]
- generic "Sponsorisé" [ref=e30]
  - heading "Learn AI in 30 days" [level=3, ref=e31]
    - link "Learn AI in 30 days" [ref=e32, url=https://www.youtube.com/watch?v=AdAdAdAdAd1&pp=x]
  - text "Sponsorisé"
- generic "27:14Transformers, the tech behind LLMs" [ref=e23] clickable [cursor:pointer]
  - heading "Transformers, the tech behind LLMs | Deep Learning Chapter 5 27 minutes" [level=3, ref=e66]
    - link "Transformers, the tech behind LLMs | Deep Learning Chapter 5 27 minutes" [ref=e117, url=https://www.youtube.com/watch?v=wjZofJX0v4M&pp=ygUYaG93]
  - link "3Blue1Brown" [ref=e177, url=https://www.youtube.com/@3blue1brown]
- generic "How a Transformer Works" [ref=e75] clickable [cursor:pointer, onclick]
  - heading "How a Transformer Works | Faraday's Law Explained" [level=3, ref=e127]
    - link "How a Transformer Works | Faraday's Law Explained" [ref=e182, url=https://www.youtube.com/shorts/KZ1s-keMRyQ]
- generic "What are Transformers" [ref=e24]
  - heading "What are Transformers (Machine Learning Model)? 5 minutes, 51 seconds" [level=3, ref=e68]
    - link "What are Transformers (Machine Learning Model)? 5 minutes, 51 seconds" [ref=e119, url=https://www.youtube.com/watch?v=ZXiruGOCn9s&list=RDZXiruGOCn9s]"#;

    #[test]
    fn youtube_videos_are_read_in_order_without_shorts_or_ads() {
        let videos = youtube_videos(&json!({ "snapshot": YOUTUBE }));
        let got: Vec<(&str, &str, &str)> = videos.iter().map(|v| (v.title.as_str(), v.channel.as_str(), v.url.as_str())).collect();
        assert_eq!(
            got,
            [
                (
                    "Comment fonctionne un transformateur – Principe de fonctionnement en génie électrique",
                    "The Engineering Mindset",
                    "https://www.youtube.com/watch?v=UchitHGF4n8"
                ),
                (
                    "Transformers, the tech behind LLMs | Deep Learning Chapter 5",
                    "3Blue1Brown",
                    "https://www.youtube.com/watch?v=wjZofJX0v4M"
                ),
                ("What are Transformers (Machine Learning Model)?", "", "https://www.youtube.com/watch?v=ZXiruGOCn9s"),
            ]
        );
        assert_eq!(
            videos[1].summary(),
            "Playing “Transformers, the tech behind LLMs | Deep Learning Chapter 5” from 3Blue1Brown on YouTube."
        );
    }

    #[test]
    fn lengths_are_taken_off_titles_and_nothing_else() {
        assert_eq!(without_length("Intro 1 heure, 2 minutes et 3 secondes"), "Intro");
        assert_eq!(without_length("Top 10 minutes of football"), "Top 10 minutes of football");
        assert_eq!(without_length("Chapter 5 27 minutes"), "Chapter 5");
        assert_eq!(video_id("https://www.youtube.com/watch?v=abc_DEF-12&t=3"), Some("abc_DEF-12".into()));
        assert_eq!(video_id("https://www.youtube.com/shorts/abc"), None);
        assert_eq!(video_id("https://www.googleadservices.com/pagead/aclk?watch?v=x"), None);
    }

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
