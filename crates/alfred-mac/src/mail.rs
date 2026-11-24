//! Mail through its own scripting: the newest email read aloud, and a reply opened with text in it.
//!
//! Alfred never sends mail. A reply opens in its own window, where Mail keeps it as a draft until the user
//! sends it or throws it away; the script has no send command at all. The sender filter and the reply's
//! text reach the script through `on run argv`, never as script source.
//!
//! Each account's inbox lists its newest message first, but Mail's combined inbox puts the accounts one
//! after the other, so the newest email is the latest of each account's first message.

use std::time::Duration;

use alfred_core::Outcome;

use crate::osascript;

/// Separates the fields of the script's answer; an email's text can't contain it.
const SEP: char = '\u{1f}';

/// `argv`: "read" or "reply", the sender filter ("" for anyone), the reply's text ("" to leave it empty).
/// Answers "" when there is no such email, else sender, subject, received, now (and, to "read", the text).
const MAIL_SCRIPT: &str = r#"on run argv
    set mode to item 1 of argv
    set fromText to item 2 of argv
    set replyText to item 3 of argv
    set nowStamp to my stamp(current date)
    tell application "Mail"
        set boxes to mailboxes of inbox
        if (count of boxes) is 0 then set boxes to {inbox}
        set newest to missing value
        repeat with mb in boxes
            try
                if fromText is "" then
                    set m to message 1 of mb
                else
                    set m to first message of mb whose sender contains fromText
                end if
                if newest is missing value then
                    set newest to m
                else if (date received of m) > (date received of newest) then
                    set newest to m
                end if
            end try
        end repeat
        if newest is missing value then return ""
        set sep to character id 31
        set subj to ""
        try
            set subj to subject of newest
        end try
        set info to (sender of newest) & sep & subj & sep & my stamp(date received of newest) & sep & nowStamp
        if mode is "reply" then
            set r to reply newest with opening window
            if replyText is not "" then set content of r to replyText
            activate
            return info
        end if
        set txt to ""
        try
            set txt to content of newest
        end try
        if (length of txt) > 4000 then set txt to text 1 thru 4000 of txt
        return info & sep & txt
    end tell
end run

on stamp(d)
    return (year of d as string) & "-" & my two((month of d) as integer) & "-" & my two(day of d) & " " & my two(hours of d) & ":" & my two(minutes of d)
end stamp

on two(n)
    if n < 10 then return "0" & n
    return n as string
end two"#;

/// An email as the script describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    /// As Mail gives it: `Ada Lovelace <ada@example.com>`.
    pub sender: String,
    pub subject: String,
    /// `YYYY-MM-DD HH:MM`, local time.
    pub received: String,
    /// The time the script ran, in the same form.
    pub now: String,
    /// The plain text, at most 4,000 characters (empty for a reply).
    pub text: String,
}

/// Read the newest email (from `from`, when given) and say who sent it, when, about what and how it begins.
pub fn read(from: Option<&str>, timeout: Duration) -> Result<Outcome, String> {
    let out = osascript(MAIL_SCRIPT, &["read", from.unwrap_or(""), ""], timeout)?;
    Ok(Outcome::new(match parse(&out) {
        Some(email) => read_summary(&email, from.is_some()),
        None => nothing_from(from),
    }))
}

/// Open a reply to the newest email (from `from`) with `text` in it, and bring it forward. Never sends.
pub fn reply(from: Option<&str>, text: &str, timeout: Duration) -> Result<Outcome, String> {
    let out = osascript(MAIL_SCRIPT, &["reply", from.unwrap_or(""), text.trim()], timeout)?;
    match parse(&out) {
        Some(email) => Ok(Outcome::new(reply_summary(&email, !text.trim().is_empty()))),
        None => Err(format!("{}, so I wrote no reply.", nothing_from(from).trim_end_matches('.'))),
    }
}

fn nothing_from(from: Option<&str>) -> String {
    match from {
        Some(f) => format!("I found no email from {f} in your inbox."),
        None => "Your inbox is empty.".into(),
    }
}

fn parse(out: &str) -> Option<Email> {
    let out = out.strip_suffix('\n').unwrap_or(out);
    if out.trim().is_empty() {
        return None;
    }
    let mut parts = out.splitn(5, SEP);
    let mut next = || parts.next().unwrap_or("").to_string();
    let email = Email { sender: next(), subject: next(), received: next(), now: next(), text: next() };
    (!email.sender.is_empty() || !email.subject.is_empty()).then_some(email)
}

pub fn read_summary(email: &Email, filtered: bool) -> String {
    let name = sender_name(&email.sender);
    let when = when(&email.received, &email.now);
    let mut s =
        if filtered { format!("The latest email from {name} came {when}") } else { format!("Your latest email is from {name}, {when}") };
    let subject = email.subject.trim();
    if !subject.is_empty() {
        s.push_str(&format!(", about “{subject}”"));
    }
    s.push('.');
    let begins = opening(&email.text, 280);
    if !begins.is_empty() {
        s.push_str(&format!(" It begins: “{begins}”"));
    }
    s
}

pub fn reply_summary(email: &Email, wrote: bool) -> String {
    let name = sender_name(&email.sender);
    let about = Some(email.subject.trim()).filter(|s| !s.is_empty()).map(|s| format!(" about “{s}”")).unwrap_or_default();
    if wrote {
        format!("I've written a reply to {name}{about}. It's open in Mail for you to check and send.")
    } else {
        format!("I've opened a reply to {name}{about} in Mail for you to write.")
    }
}

/// The name to say: "Ada Lovelace <ada@example.com>" → "Ada Lovelace"; a bare address stays itself.
pub fn sender_name(sender: &str) -> String {
    let s = sender.trim();
    let name = match s.find('<') {
        Some(at) if at > 0 => s[..at].trim(),
        Some(_) => s.trim_matches(['<', '>']),
        None => s,
    };
    let name = name.trim_matches(['"', '\'']).trim();
    if name.is_empty() { "someone".into() } else { name.to_string() }
}

/// "today at 08:00", "yesterday at 21:10", "on 23 November", "on 4 March 2025".
pub fn when(received: &str, now: &str) -> String {
    let (Some((d, time)), Some((today, _))) = (received.split_once(' '), now.split_once(' ')) else {
        return "recently".into();
    };
    match (day_number(d), day_number(today)) {
        (Some(a), Some(b)) if a == b => format!("today at {time}"),
        (Some(a), Some(b)) if a + 1 == b => format!("yesterday at {time}"),
        _ => {
            let field = |i: usize| d.split('-').nth(i).and_then(|x| x.parse::<usize>().ok()).unwrap_or(0);
            const MONTHS: [&str; 12] =
                ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
            let month = MONTHS.get(field(1).wrapping_sub(1)).copied().unwrap_or("");
            let year = if d.get(..4) == today.get(..4) { String::new() } else { format!(" {}", field(0)) };
            format!("on {} {month}{year}", field(2))
        }
    }
}

/// Days since 1970-01-01 for "YYYY-MM-DD" (the civil-calendar formula), to tell yesterday across a month.
fn day_number(date: &str) -> Option<i64> {
    let mut it = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Characters with nothing to say: an image's placeholder (￼) and the invisible padding newsletters put
/// after their first line.
const SILENT: &[char] =
    &['\u{fffc}', '\u{034f}', '\u{00ad}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{200e}', '\u{200f}', '\u{2060}', '\u{feff}'];

/// The start of an email worth saying: its own words before any quoted reply, without links or image
/// placeholders, on one line, cut at a word within `max` characters.
pub fn opening(text: &str, max: usize) -> String {
    let text: String = text.chars().filter(|c| !SILENT.contains(c)).collect();
    let mut words: Vec<&str> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let quoted = l.starts_with('>')
            || l.starts_with("-----Original Message")
            || (l.starts_with("On ") && l.ends_with("wrote:"))
            || (l.starts_with("Le ") && (l.ends_with("a écrit :") || l.ends_with("a écrit:")));
        if quoted {
            break;
        }
        words.extend(l.split_whitespace().filter(|w| !w.starts_with("http") && !w.starts_with("<http") && !w.starts_with('[')));
    }
    let mut out = String::new();
    for w in words {
        if out.chars().count() + w.chars().count() + 1 > max {
            out.push('…');
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(text: &str) -> Email {
        Email {
            sender: "Ada Lovelace <ada@example.com>".into(),
            subject: "Lunch on Friday?".into(),
            received: "2026-11-24 08:05".into(),
            now: "2026-11-24 09:41".into(),
            text: text.into(),
        }
    }

    #[test]
    fn the_script_takes_text_only_through_argv_and_cannot_send() {
        assert!(MAIL_SCRIPT.starts_with("on run argv"));
        assert!(!MAIL_SCRIPT.contains("{}"));
        let words: Vec<&str> = MAIL_SCRIPT.split(|c: char| !c.is_alphanumeric()).collect();
        assert!(!words.contains(&"send"), "no send command, ever");
        assert!(words.contains(&"argv"));
    }

    #[test]
    fn the_answer_is_split_into_its_fields() {
        let out = format!("Ada <ada@example.com>{SEP}Hi{SEP}2026-11-24 08:05{SEP}2026-11-24 09:41{SEP}Line one\nwith {SEP} inside\n");
        let e = parse(&out).unwrap();
        assert_eq!((e.sender.as_str(), e.subject.as_str(), e.received.as_str()), ("Ada <ada@example.com>", "Hi", "2026-11-24 08:05"));
        assert_eq!(e.text, format!("Line one\nwith {SEP} inside"), "the text keeps whatever it holds");
        assert_eq!(parse("\n"), None, "no such email");
    }

    #[test]
    fn senders_are_said_by_name() {
        assert_eq!(sender_name("Ada Lovelace <ada@example.com>"), "Ada Lovelace");
        assert_eq!(sender_name("\"Ada\" <ada@example.com>"), "Ada");
        assert_eq!(sender_name("<ada@example.com>"), "ada@example.com");
        assert_eq!(sender_name("ada@example.com"), "ada@example.com");
        assert_eq!(sender_name(""), "someone");
    }

    #[test]
    fn dates_are_said_relative_to_today() {
        assert_eq!(when("2026-11-24 08:05", "2026-11-24 09:41"), "today at 08:05");
        assert_eq!(when("2026-11-30 23:59", "2026-12-01 00:10"), "yesterday at 23:59", "across a month");
        assert_eq!(when("2026-11-20 10:00", "2026-11-24 09:41"), "on 20 November");
        assert_eq!(when("2025-03-04 10:00", "2026-11-24 09:41"), "on 4 March 2025");
        assert_eq!(when("", ""), "recently");
    }

    #[test]
    fn the_opening_skips_quotes_and_links() {
        let text = "Are you free for lunch on Friday?\n\nSee https://example.com/menu [image]\n\nOn Mon, Bob wrote:\n> old text";
        assert_eq!(opening(text, 280), "Are you free for lunch on Friday? See");
        let long = opening(&"word ".repeat(200), 40);
        assert!(long.ends_with('…') && long.chars().count() <= 41, "{long}");
        assert_eq!(opening("Merci !\nLe 3 mars, Marie a écrit :\n> avant", 100), "Merci !");
        assert_eq!(opening("\u{fffc} Hello\u{200c} \u{034f} \u{fffc} there", 100), "Hello there", "seen in a real newsletter");
    }

    #[test]
    fn summaries_say_who_when_and_what() {
        assert_eq!(
            read_summary(&email("Are you free?"), false),
            "Your latest email is from Ada Lovelace, today at 08:05, about “Lunch on Friday?”. It begins: “Are you free?”"
        );
        assert_eq!(read_summary(&email(""), true), "The latest email from Ada Lovelace came today at 08:05, about “Lunch on Friday?”.");
        assert_eq!(
            reply_summary(&email(""), true),
            "I've written a reply to Ada Lovelace about “Lunch on Friday?”. It's open in Mail for you to check and send."
        );
        assert_eq!(
            reply_summary(&email(""), false),
            "I've opened a reply to Ada Lovelace about “Lunch on Friday?” in Mail for you to write."
        );
        assert_eq!(nothing_from(Some("Bob")), "I found no email from Bob in your inbox.");
    }
}
