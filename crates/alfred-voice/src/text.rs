//! Preparing text for speech: drop what can't be said, and cut it into sentences so the first one plays
//! while the rest are synthesized.

/// Text a voice can say: no markdown, links read as "a link", emoji and symbols dropped.
pub fn speakable(text: &str) -> String {
    let mut words = Vec::new();
    for w in text.split_whitespace() {
        let lower = w.to_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("www.") {
            words.push("a link".to_string());
            continue;
        }
        let cleaned: String = w
            .chars()
            .filter(|c| !matches!(c, '*' | '_' | '`' | '#' | '~' | '|' | '<' | '>' | '[' | ']' | '{' | '}'))
            .filter(|c| !is_emoji_or_symbol(*c))
            .map(|c| match c {
                '→' => ' ',
                '&' => '+',
                c => c,
            })
            .collect();
        let cleaned = cleaned.replace('+', " and ");
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            words.push(cleaned.to_string());
        }
    }
    words.join(" ").replace(" - ", ", ").replace("  ", " ")
}

fn is_emoji_or_symbol(c: char) -> bool {
    let u = c as u32;
    (0x1F000..=0x1FAFF).contains(&u) || (0x2600..=0x27BF).contains(&u) || (0x2B00..=0x2BFF).contains(&u) || u == 0xFE0F || u == 0x200D
}

/// Sentences for synthesis: split after `.`, `!`, `?` and line breaks, short fragments merged with the
/// next, long sentences split at commas. A long first sentence is cut short (see [`split_first`]), so
/// the first audio is ready sooner.
pub fn sentences(text: &str) -> Vec<String> {
    const SHORT: usize = 16;
    const LONG: usize = 220;
    let mut raw = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            push(&mut raw, &mut cur);
            continue;
        }
        cur.push(c);
        let next = chars.get(i + 1).copied();
        if matches!(c, '.' | '!' | '?' | '…') && next.is_none_or(|n| n.is_whitespace() || n == '”' || n == '"') {
            // Keep "e.g." and "3.5" together: only split before whitespace or the end.
            push(&mut raw, &mut cur);
        }
    }
    push(&mut raw, &mut cur);
    // Merge short fragments forward so "Very good." doesn't become its own clipped utterance.
    let mut merged: Vec<String> = Vec::new();
    let mut carry = String::new();
    for s in raw {
        let joined = if carry.is_empty() { s } else { format!("{carry} {s}") };
        if joined.chars().count() < SHORT {
            carry = joined;
        } else {
            merged.push(joined);
            carry.clear();
        }
    }
    if !carry.is_empty() {
        match merged.last_mut() {
            Some(last) => {
                last.push(' ');
                last.push_str(&carry);
            }
            None => merged.push(carry),
        }
    }
    let mut out: Vec<String> = merged.into_iter().flat_map(|s| split_long(&s, LONG)).collect();
    if let Some(first) = out.first().cloned() {
        out.splice(0..1, split_first(&first));
    }
    out
}

/// The first sentence in one or two parts. Kokoro's time grows with the length of what it says, so a
/// first sentence of more than [`FIRST_MAX_WORDS`] words is cut: at the first comma, semicolon, colon
/// or sentence end (where a short sentence was merged in) that leaves 3 to [`FIRST_MAX_WORDS`] words
/// before it, else after [`FIRST_CUT_WORDS`] words.
pub fn split_first(sentence: &str) -> Vec<String> {
    let words: Vec<&str> = sentence.split_whitespace().collect();
    if words.len() <= FIRST_MAX_WORDS {
        return vec![sentence.to_string()];
    }
    let at = (3..=FIRST_MAX_WORDS).find(|&n| words[n - 1].ends_with([',', ';', ':', '.', '!', '?'])).unwrap_or(FIRST_CUT_WORDS);
    vec![words[..at].join(" "), words[at..].join(" ")]
}

/// A first sentence longer than this many words is cut.
const FIRST_MAX_WORDS: usize = 8;
/// Where a first sentence with no early comma is cut.
const FIRST_CUT_WORDS: usize = 6;

fn push(out: &mut Vec<String>, cur: &mut String) {
    let s = cur.trim();
    if !s.is_empty() {
        out.push(s.to_string());
    }
    cur.clear();
}

fn split_long(s: &str, max: usize) -> Vec<String> {
    if s.chars().count() <= max {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for part in s.split_inclusive([',', ';', ':']) {
        if !cur.is_empty() && cur.chars().count() + part.chars().count() > max {
            out.push(cur.trim().to_string());
            cur.clear();
        }
        cur.push_str(part);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speakable_drops_markup_links_and_emoji() {
        assert_eq!(speakable("**Done!** See https://example.com 🎉"), "Done! See a link");
        assert_eq!(speakable("Milk & eggs → `fridge`"), "Milk and eggs fridge");
        assert_eq!(speakable("# Title\n- item one - item two"), "Title, item one, item two");
    }

    #[test]
    fn sentences_split_and_merge() {
        assert_eq!(
            sentences("Very good. I've saved your note about the plumber. Is there anything else I can do?"),
            vec!["Very good. I've saved your note", "about the plumber.", "Is there anything else I can do?"]
        );
        // A short tail rides with the sentence before it, unless that makes the first one too long.
        assert_eq!(sentences("I have saved it. Anything else?"), vec!["I have saved it. Anything else?"]);
        assert_eq!(
            sentences("I've saved your note about the plumber. Anything else?"),
            vec!["I've saved your note about the plumber.", "Anything else?"]
        );
        assert_eq!(sentences("Version 3.5 is out. Yes."), vec!["Version 3.5 is out. Yes."]);
        assert_eq!(sentences(""), Vec::<String>::new());
        assert_eq!(sentences("Hi."), vec!["Hi."]);
    }

    #[test]
    fn a_long_first_sentence_is_cut_short() {
        // At the first comma that leaves 3 to 8 words before it.
        assert_eq!(
            sentences("Of course, I have opened Notes, and your shopping list now has milk and eggs. It is saved in iCloud."),
            vec!["Of course, I have opened Notes,", "and your shopping list now has milk and eggs.", "It is saved in iCloud."]
        );
        // With no comma there, after six words; later sentences stay whole.
        assert_eq!(
            sentences("The meeting with the plumber is tomorrow at nine in the morning. I will remind you an hour before it starts."),
            vec!["The meeting with the plumber is", "tomorrow at nine in the morning.", "I will remind you an hour before it starts."]
        );
        // A comma too late to help doesn't count.
        assert_eq!(
            split_first("I found three emails from your bank about the new card, all unread."),
            vec!["I found three emails from your", "bank about the new card, all unread."]
        );
        // Eight words or fewer stay as they are.
        assert_eq!(sentences("Notes is open."), vec!["Notes is open."]);
        assert_eq!(split_first("I have saved your note about the plumber."), vec!["I have saved your note about the plumber."]);
    }

    #[test]
    fn long_sentences_split_at_commas() {
        let long = format!("{}, {}, {}.", "a".repeat(120), "b".repeat(120), "c".repeat(30));
        let parts = sentences(&long);
        assert!(parts.len() >= 2, "{parts:?}");
        assert!(parts.iter().all(|p| p.chars().count() <= 220));
    }
}
