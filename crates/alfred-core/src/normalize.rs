//! Turning loose words into well-formed action arguments: app names, links, note titles, search URLs.
//! Shared by the router and by argument repair, so both paths produce the same actions.

/// Spoken or short names mapped to the app's real name.
const APP_ALIASES: &[(&str, &str)] = &[
    ("app store", "App Store"),
    ("apple music", "Music"),
    ("arc", "Arc"),
    ("brave", "Brave Browser"),
    ("calculator", "Calculator"),
    ("calendar", "Calendar"),
    ("chrome", "Google Chrome"),
    ("code", "Visual Studio Code"),
    ("contacts", "Contacts"),
    ("discord", "Discord"),
    ("excel", "Microsoft Excel"),
    ("facetime", "FaceTime"),
    ("finder", "Finder"),
    ("firefox", "Firefox"),
    ("google chrome", "Google Chrome"),
    ("iterm", "iTerm"),
    ("keynote", "Keynote"),
    ("mail", "Mail"),
    ("maps", "Maps"),
    ("messages", "Messages"),
    ("music", "Music"),
    ("notes", "Notes"),
    ("numbers", "Numbers"),
    ("outlook", "Microsoft Outlook"),
    ("pages", "Pages"),
    ("photos", "Photos"),
    ("podcasts", "Podcasts"),
    ("preview", "Preview"),
    ("reminders", "Reminders"),
    ("safari", "Safari"),
    ("settings", "System Settings"),
    ("slack", "Slack"),
    ("spotify", "Spotify"),
    ("system preferences", "System Settings"),
    ("system settings", "System Settings"),
    ("teams", "Microsoft Teams"),
    ("terminal", "Terminal"),
    ("textedit", "TextEdit"),
    ("vs code", "Visual Studio Code"),
    ("vscode", "Visual Studio Code"),
    ("whatsapp", "WhatsApp"),
    ("word", "Microsoft Word"),
    ("xcode", "Xcode"),
    ("zoom", "zoom.us"),
];

/// Names people say as if they were apps but which are websites.
const SITES: &[(&str, &str)] = &[
    ("chatgpt", "https://chatgpt.com"),
    ("github", "https://github.com"),
    ("gmail", "https://mail.google.com"),
    ("google", "https://www.google.com"),
    ("google maps", "https://maps.google.com"),
    ("hacker news", "https://news.ycombinator.com"),
    ("linkedin", "https://www.linkedin.com"),
    ("netflix", "https://www.netflix.com"),
    ("reddit", "https://www.reddit.com"),
    ("twitter", "https://x.com"),
    ("wikipedia", "https://www.wikipedia.org"),
    ("youtube", "https://www.youtube.com"),
];

const BROWSERS: &[&str] = &["safari", "chrome", "google chrome", "firefox", "arc", "brave", "the browser", "browser", "my browser"];

/// The app's display name: an alias when one is known, else the words title-cased.
/// Returns `None` for something that cannot be an app name (empty, or a path).
pub fn app_name(raw: &str) -> Option<String> {
    let cleaned = strip_articles(raw.trim().trim_matches(['"', '\'', '“', '”', '.', ',']).trim());
    let cleaned = strip_suffix_ci(cleaned, " application").or_else(|| strip_suffix_ci(cleaned, " app")).unwrap_or(cleaned).trim();
    let lower = cleaned.to_lowercase();
    if cleaned.is_empty()
        || cleaned.contains('/')
        || cleaned.contains('\\')
        || cleaned.len() > 60
        || ["app", "application", "it", "this", "that"].contains(&lower.as_str())
    {
        return None;
    }
    if let Some((_, real)) = APP_ALIASES.iter().find(|(k, _)| *k == lower) {
        return Some((*real).to_string());
    }
    // Keep names that already carry capitals ("iMovie", "OmniFocus"); title-case lowercase ones.
    if cleaned.chars().any(|c| c.is_uppercase()) {
        return Some(cleaned.to_string());
    }
    Some(cleaned.split_whitespace().map(capitalize).collect::<Vec<_>>().join(" "))
}

/// Whether Alfred knows this app by name (one of its aliases or their real names).
pub fn is_known_app(raw: &str) -> bool {
    let lower = strip_articles(raw.trim()).to_lowercase();
    let lower = lower.strip_suffix(" app").unwrap_or(&lower);
    APP_ALIASES.iter().any(|(alias, real)| *alias == lower || real.to_lowercase() == lower)
}

/// The website a spoken name stands for: "youtube" → "https://www.youtube.com".
pub fn site_url(name: &str) -> Option<String> {
    let lower = strip_articles(name.trim()).to_lowercase();
    SITES.iter().find(|(k, _)| *k == lower).map(|(_, url)| (*url).to_string())
}

/// The browser app a phrase names, if it names one: "chrome" → "Google Chrome". "the browser" → `Some(None)`
/// (the default browser); not a browser → `None`.
pub fn browser_name(raw: &str) -> Option<Option<String>> {
    let lower = raw.trim().to_lowercase();
    if !BROWSERS.contains(&lower.as_str()) {
        return None;
    }
    if lower.contains("browser") {
        return Some(None);
    }
    Some(app_name(&lower))
}

/// A well-formed http(s) URL from what the user said, or `None` when it isn't a link.
/// "example.com" → "https://example.com"; "ftp://x" and "javascript:…" → None.
pub fn url(raw: &str) -> Option<String> {
    let s = raw.trim().trim_matches(['"', '\'', '<', '>', '“', '”']).trim().trim_end_matches(['.', ',']);
    if s.is_empty() || s.contains(char::is_whitespace) {
        return None;
    }
    let lower = s.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return (s.len() > lower.find("://").unwrap_or(0) + 3).then(|| s.to_string());
    }
    if lower.contains(':') {
        return None; // another scheme (ftp:, file:, javascript:) or a port without scheme
    }
    looks_like_domain(s).then(|| format!("https://{s}"))
}

/// "example.com", "docs.rs/foo", "www.apple.com/mac".
pub fn looks_like_domain(s: &str) -> bool {
    let host = s.split('/').next().unwrap_or(s);
    let mut parts = host.split('.');
    let Some(first) = parts.next() else { return false };
    let rest: Vec<&str> = parts.collect();
    let Some(tld) = rest.last() else { return false };
    !first.is_empty()
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && rest.iter().all(|p| !p.is_empty())
        && (2..=12).contains(&tld.len())
        && tld.chars().all(|c| c.is_ascii_alphabetic())
}

/// A short title for a note: its first line or sentence, at most `max_words` words, capitalized.
pub fn title_from(body: &str, max_words: usize) -> String {
    let first = body.trim().split(['\n', '.', '!', '?', ';']).find(|s| !s.trim().is_empty()).unwrap_or("Note").trim();
    // "Groceries: milk, eggs" → "Groceries": a short lead before a colon is the title the user meant.
    let first = match first.split_once(": ") {
        Some((head, rest)) if !rest.trim().is_empty() && (1..=max_words).contains(&head.split_whitespace().count()) => head.trim(),
        _ => first,
    };
    let words: Vec<&str> = first.split_whitespace().collect();
    let mut title = words.iter().take(max_words).copied().collect::<Vec<_>>().join(" ");
    if words.len() > max_words {
        title.push('…');
    }
    capitalize(title.trim_end_matches([',', ':', '-']))
}

/// The search page for `query` on `engine` ("google", "duckduckgo", "bing"; anything else is Google).
pub fn search_url(engine: &str, query: &str) -> String {
    let q = form_encode(query.trim());
    match engine.to_lowercase().as_str() {
        "duckduckgo" | "ddg" => format!("https://duckduckgo.com/?q={q}"),
        "bing" => format!("https://www.bing.com/search?q={q}"),
        _ => format!("https://www.google.com/search?q={q}"),
    }
}

/// YouTube's results for `query`.
pub fn youtube_search_url(query: &str) -> String {
    format!("https://www.youtube.com/results?search_query={}", form_encode(query.trim()))
}

/// `application/x-www-form-urlencoded`: unreserved bytes stay, spaces become '+', the rest is %XX.
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// First letter upper-cased, the rest untouched.
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn strip_articles(s: &str) -> &str {
    for a in ["the ", "my ", "a ", "an "] {
        if let Some(rest) = strip_prefix_ci(s, a) {
            return rest.trim_start();
        }
    }
    s
}

/// `s` without `prefix`, compared case-insensitively (ASCII). Keeps the original casing of the rest.
pub fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let n = prefix.len();
    (s.len() >= n && s.is_char_boundary(n) && s[..n].eq_ignore_ascii_case(prefix)).then(|| &s[n..])
}

/// `s` without `suffix`, compared case-insensitively (ASCII).
pub fn strip_suffix_ci<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let n = suffix.len();
    (s.len() >= n && s.is_char_boundary(s.len() - n) && s[s.len() - n..].eq_ignore_ascii_case(suffix)).then(|| &s[..s.len() - n])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_resolve_aliases_and_title_case() {
        assert_eq!(app_name("notes").as_deref(), Some("Notes"));
        assert_eq!(app_name("the Notes app").as_deref(), Some("Notes"));
        assert_eq!(app_name("chrome").as_deref(), Some("Google Chrome"));
        assert_eq!(app_name("vs code").as_deref(), Some("Visual Studio Code"));
        assert_eq!(app_name("activity monitor").as_deref(), Some("Activity Monitor"));
        assert_eq!(app_name("iMovie").as_deref(), Some("iMovie"));
    }

    #[test]
    fn app_names_refuse_paths_and_empties() {
        assert_eq!(app_name("/Applications/Evil.app"), None);
        assert_eq!(app_name("  "), None);
        assert_eq!(app_name("the app"), None);
    }

    #[test]
    fn sites_and_browsers() {
        assert_eq!(site_url("YouTube").as_deref(), Some("https://www.youtube.com"));
        assert_eq!(site_url("notes"), None);
        assert_eq!(browser_name("chrome"), Some(Some("Google Chrome".into())));
        assert_eq!(browser_name("the browser"), Some(None));
        assert_eq!(browser_name("notes"), None);
    }

    #[test]
    fn urls_are_http_only() {
        assert_eq!(url("example.com").as_deref(), Some("https://example.com"));
        assert_eq!(url("https://docs.rs/rune").as_deref(), Some("https://docs.rs/rune"));
        assert_eq!(url("www.apple.com/mac.").as_deref(), Some("https://www.apple.com/mac"));
        assert_eq!(url("javascript:alert(1)"), None);
        assert_eq!(url("file:///etc/passwd"), None);
        assert_eq!(url("ftp://example.com"), None);
        assert_eq!(url("notes"), None);
        assert_eq!(url("two words.com"), None);
        assert_eq!(url("https://"), None);
    }

    #[test]
    fn titles_come_from_the_first_sentence() {
        assert_eq!(title_from("buy milk and eggs. Also bread", 6), "Buy milk and eggs");
        assert_eq!(title_from("the plumber comes at 3 on thursday afternoon", 6), "The plumber comes at 3 on…");
        assert_eq!(title_from("   ", 6), "Note");
        assert_eq!(title_from("Groceries: milk, eggs and bread", 6), "Groceries");
        assert_eq!(title_from("at 10:30 the meeting starts", 6), "At 10:30 the meeting starts", "a colon inside a time is not a title");
    }

    #[test]
    fn search_urls_are_encoded() {
        assert_eq!(search_url("google", "rust rune ui"), "https://www.google.com/search?q=rust+rune+ui");
        assert_eq!(search_url("duckduckgo", "a&b=c"), "https://duckduckgo.com/?q=a%26b%3Dc");
        assert_eq!(search_url("bing", "café"), "https://www.bing.com/search?q=caf%C3%A9");
    }

    #[test]
    fn case_insensitive_affixes_keep_casing() {
        assert_eq!(strip_prefix_ci("Open Notes", "open "), Some("Notes"));
        assert_eq!(strip_suffix_ci("buy Milk In Notes", " in notes"), Some("buy Milk"));
        assert_eq!(strip_prefix_ci("é", "e"), None);
    }
}
