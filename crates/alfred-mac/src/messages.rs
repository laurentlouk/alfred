//! Texts through Messages: the person is found in Contacts, and once the user says yes the message goes
//! to their existing conversation (so an SMS conversation stays SMS), or else over iMessage.
//!
//! Finding someone never sends anything: [`find`] answers with an [`Action::SendText`] for the agent to
//! confirm, and only [`send`] sends. Names, numbers and the message reach the scripts through
//! `on run argv`, never as script source.

use std::time::Duration;

use alfred_core::Action;
use alfred_core::catalog::fold;

use crate::osascript;

const US: char = '\u{1f}';
const RS: char = '\u{1e}';
const GS: char = '\u{1d}';

/// `argv`: part of a name. Up to five people whose name or nickname has it: each person is their name,
/// then one record per phone or email (`phone` or `email`, its label, its value); people end with GS.
const CONTACTS_SCRIPT: &str = r#"on run argv
    set q to item 1 of argv
    set us to character id 31
    set rs to character id 30
    set gs to character id 29
    set out to ""
    tell application "Contacts"
        set found to {}
        try
            set found to (people whose name contains q)
        end try
        if found is {} then
            try
                set found to (people whose nickname contains q)
            end try
        end if
        set n to 0
        repeat with p in found
            set n to n + 1
            if n > 5 then exit repeat
            set who to ""
            try
                set who to name of p
            end try
            set out to out & who
            repeat with ph in phones of p
                set lbl to ""
                try
                    set lbl to label of ph
                end try
                set out to out & rs & "phone" & us & lbl & us & (value of ph)
            end repeat
            repeat with em in emails of p
                set lbl to ""
                try
                    set lbl to label of em
                end try
                set out to out & rs & "email" & us & lbl & us & (value of em)
            end repeat
            set out to out & gs
        end repeat
    end tell
    return out
end run"#;

/// `argv`: the handle (a number or an address), the message, and the number's last digits ("" for an
/// address). A one-to-one conversation with that handle gets it, keeping its service; else iMessage, or
/// SMS when the Mac has no iMessage account.
const SEND_SCRIPT: &str = r#"on run argv
    set target to item 1 of argv
    set msg to item 2 of argv
    set digits to item 3 of argv
    tell application "Messages"
        repeat with c in chats
            try
                set ps to participants of c
                if (count of ps) is 1 then
                    set h to handle of item 1 of ps
                    if h is target or (digits is not "" and h ends with digits) then
                        send msg to c
                        return "chat"
                    end if
                end if
            end try
        end repeat
        set svc to missing value
        try
            set svc to first account whose service type is iMessage and enabled is true
        end try
        if svc is missing value then set svc to first account whose service type is SMS
        send msg to participant target of svc
        return "new"
    end tell
end run"#;

/// A person in Contacts and how to reach them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub handles: Vec<Handle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handle {
    pub phone: bool,
    /// As Contacts names it, cleaned: "mobile", "iPhone", "home".
    pub label: String,
    pub value: String,
}

/// Who a text would go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// The text to confirm.
    One(Action),
    /// Several people match: their names.
    Several(Vec<String>),
    /// The person has no number or address.
    NoHandle(String),
    Nobody,
}

/// Find who `to` is and prepare the text to them; nothing is sent.
pub fn find(to: &str, text: &str, timeout: Duration) -> Result<Found, String> {
    if let Some(handle) = direct_handle(to) {
        return Ok(Found::One(Action::SendText { name: to.trim().to_string(), label: String::new(), handle, text: text.to_string() }));
    }
    let out = osascript(CONTACTS_SCRIPT, &[to.trim()], timeout)?;
    Ok(choose(to, &parse_people(&out), text))
}

/// Send `text` to `handle`. Only ever run after the user's yes.
pub fn send(handle: &str, text: &str, timeout: Duration) -> Result<(), String> {
    osascript(SEND_SCRIPT, &[handle, text, &last_digits(handle)], timeout).map(|_| ())
}

/// A number or an address the user said as is: "+33 6 12 34 56 78", "ada@example.com".
pub fn direct_handle(to: &str) -> Option<String> {
    let t = to.trim();
    if t.contains('@') && t.contains('.') && !t.contains(' ') {
        return Some(t.to_string());
    }
    let digits = t.chars().filter(char::is_ascii_digit).count();
    let phone_chars = t.chars().all(|c| c.is_ascii_digit() || " +-.()".contains(c));
    (digits >= 6 && phone_chars).then(|| phone_handle(t))
}

/// A number as Messages takes it: its digits, after a leading `+` when it has one.
fn phone_handle(raw: &str) -> String {
    let plus = raw.trim_start().starts_with('+');
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if plus { format!("+{digits}") } else { digits }
}

/// The last nine digits of a number, to recognize it whatever its country prefix; "" for an address.
fn last_digits(handle: &str) -> String {
    if handle.contains('@') {
        return String::new();
    }
    let digits: Vec<char> = handle.chars().filter(char::is_ascii_digit).collect();
    digits[digits.len().saturating_sub(9)..].iter().collect()
}

pub fn parse_people(out: &str) -> Vec<Person> {
    out.trim_end_matches(['\n', '\r'])
        .split(GS)
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let mut records = p.split(RS);
            let name = records.next().unwrap_or("").trim().to_string();
            let handles = records
                .filter_map(|r| {
                    let mut f = r.split(US);
                    let (kind, label, value) = (f.next()?, f.next().unwrap_or(""), f.next().unwrap_or("").trim());
                    (!value.is_empty()).then(|| Handle { phone: kind == "phone", label: clean_label(label), value: value.to_string() })
                })
                .collect();
            Person { name, handles }
        })
        .collect()
}

/// Contacts' stored labels ("_$!<Mobile>!$_") as people say them ("mobile").
fn clean_label(raw: &str) -> String {
    let l = raw.trim().trim_start_matches("_$!<").trim_end_matches(">!$_").trim();
    if l.eq_ignore_ascii_case("iphone") { "iPhone".into() } else { l.to_lowercase() }
}

/// The person `to` means among `people`, and their best way to get a text: a mobile number, then any
/// number, then an email address.
pub fn choose(to: &str, people: &[Person], text: &str) -> Found {
    let want = fold(to);
    let exact: Vec<&Person> =
        people.iter().filter(|p| fold(&p.name) == want || fold(&p.name).split(' ').next() == Some(want.as_str())).collect();
    let person = match (people, exact.as_slice()) {
        ([], _) => return Found::Nobody,
        ([only], _) => only,
        (_, [only]) => *only,
        _ => return Found::Several(people.iter().take(3).map(|p| p.name.clone()).collect()),
    };
    let mobile = |h: &&Handle| h.phone && ["mobile", "iphone", "portable", "cell", "cellulaire"].contains(&h.label.to_lowercase().as_str());
    let Some(h) = person.handles.iter().find(mobile).or_else(|| person.handles.iter().find(|h| h.phone)).or(person.handles.first()) else {
        return Found::NoHandle(person.name.clone());
    };
    let label = match (h.phone, h.label.as_str()) {
        (true, "" | "main" | "other" | "autre") => "number".to_string(),
        (true, l) => l.to_string(),
        (false, "" | "other" | "autre") => "email".to_string(),
        (false, l) => format!("{l} email"),
    };
    let handle = if h.phone { phone_handle(&h.value) } else { h.value.clone() };
    Found::One(Action::SendText { name: person.name.clone(), label, handle, text: text.to_string() })
}

/// What Alfred says when it can't pick one person.
pub fn several(to: &str, names: &[String]) -> String {
    let list = match names {
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => String::new(),
    };
    format!("Several people in your contacts match “{to}”: {list}. Say the full name and I'll write it.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str, handles: &[(bool, &str, &str)]) -> Person {
        Person {
            name: name.into(),
            handles: handles.iter().map(|(phone, l, v)| Handle { phone: *phone, label: l.to_string(), value: v.to_string() }).collect(),
        }
    }

    fn text(found: Found) -> (String, String, String) {
        match found {
            Found::One(Action::SendText { name, label, handle, .. }) => (name, label, handle),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_scripts_take_text_only_through_argv() {
        for s in [CONTACTS_SCRIPT, SEND_SCRIPT] {
            // Constant scripts (AppleScript's `{}` is an empty list, not a format hole) that run no shell.
            assert!(s.starts_with("on run argv") && !s.contains("do shell script") && s.is_ascii(), "{s}");
        }
        assert!(!CONTACTS_SCRIPT.contains("send"), "finding someone never sends");
    }

    #[test]
    fn the_scripts_compile() {
        if !cfg!(target_os = "macos") {
            return;
        }
        // Compiling reads the apps' dictionaries without running anything in them.
        for (name, s) in [("contacts", CONTACTS_SCRIPT), ("send", SEND_SCRIPT)] {
            let out = std::env::temp_dir().join(format!("alfred-{name}-{}.scpt", std::process::id()));
            let mut cmd = std::process::Command::new("osacompile");
            cmd.arg("-e").arg(s).arg("-o").arg(&out);
            let result = crate::run(cmd, Duration::from_secs(30));
            std::fs::remove_file(&out).ok();
            assert!(result.is_ok(), "{name}: {result:?}");
        }
    }

    #[test]
    fn contacts_answers_are_read_person_by_person() {
        let out = format!(
            "Ada Lovelace{RS}phone{US}_$!<Mobile>!$_{US}06 12 34 56 78{RS}email{US}_$!<Home>!$_{US}ada@example.com{GS}Charles{GS}\n"
        );
        let people = parse_people(&out);
        assert_eq!(people.len(), 2);
        assert_eq!(people[0].handles[0], Handle { phone: true, label: "mobile".into(), value: "06 12 34 56 78".into() });
        assert_eq!(people[0].handles[1].label, "home");
        assert!(people[1].handles.is_empty());
        assert!(parse_people("\n").is_empty());
    }

    #[test]
    fn a_mobile_number_is_chosen_first() {
        let ada = person(
            "Ada Lovelace",
            &[(false, "home", "ada@example.com"), (true, "work", "01 23 45 67 89"), (true, "iPhone", "+33 6 12 34 56 78")],
        );
        assert_eq!(text(choose("Ada", &[ada], "Hi")), ("Ada Lovelace".into(), "iPhone".into(), "+33612345678".into()));
        let office = person("Bob", &[(true, "", "01 23 45 67 89")]);
        assert_eq!(text(choose("Bob", &[office], "Hi")).1, "number");
        let mail_only = person("Grace", &[(false, "work", "grace@example.com")]);
        assert_eq!(text(choose("Grace", &[mail_only], "Hi")), ("Grace".into(), "work email".into(), "grace@example.com".into()));
        assert_eq!(choose("Zed", &[person("Zed", &[])], "Hi"), Found::NoHandle("Zed".into()));
        assert_eq!(choose("Zed", &[], "Hi"), Found::Nobody);
    }

    #[test]
    fn several_matches_are_named_unless_one_is_exact() {
        let ada = person("Ada Lovelace", &[(true, "mobile", "0612345678")]);
        let adam = person("Adam Smith", &[(true, "mobile", "0699999999")]);
        let byron = person("Ada Byron", &[(true, "mobile", "0611111111")]);
        assert_eq!(text(choose("ada", &[ada.clone(), adam], "Hi")).0, "Ada Lovelace", "the first name said exactly");
        assert_eq!(choose("ada", &[ada, byron], "Hi"), Found::Several(vec!["Ada Lovelace".into(), "Ada Byron".into()]));
        assert_eq!(
            several("Ada", &["Ada Lovelace".into(), "Ada Byron".into()]),
            "Several people in your contacts match “Ada”: Ada Lovelace and Ada Byron. Say the full name and I'll write it."
        );
    }

    #[test]
    fn a_number_or_address_said_is_used_as_is() {
        assert_eq!(direct_handle("+33 6 12 34 56 78").as_deref(), Some("+33612345678"));
        assert_eq!(direct_handle("06.12.34.56.78").as_deref(), Some("0612345678"));
        assert_eq!(direct_handle("ada@example.com").as_deref(), Some("ada@example.com"));
        assert_eq!(direct_handle("Ada"), None);
        assert_eq!(direct_handle("Room 12"), None);
        assert_eq!(last_digits("+33612345678"), "612345678", "matches 06 12 34 56 78 in a conversation");
        assert_eq!(last_digits("ada@example.com"), "");
    }
}
