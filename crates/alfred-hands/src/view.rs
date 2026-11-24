//! An app's accessibility tree, compacted into lines a model can read in a few thousand tokens:
//!
//! ```text
//! window "Inbox – 2 messages"
//!  e1 button "Reply"
//!  e3 row "Marie Curie, Dinner on Friday?" [selected]
//!  text "Are you free for dinner on Friday?"
//! ```
//!
//! Lines with a ref (`e1`) can be acted on; `text` lines are content to read. Refs are shown without their
//! snapshot prefix and mapped back when the model picks one; menu-bar refs start with `m` so they never
//! collide with the window's.
//!
//! Real apps are big (Mail's window holds hundreds of elements), so the character budget is shared
//! fairly: at every level each branch gets an equal part of what is left, and a small branch gives what it
//! doesn't use to the others. A long sidebar can't crowd out the message list beside it.
//!
//! Apps share patterns: a chat has a message box to type in, a search field and a send button. These are
//! named first, under `KEY PLACES`, so the model goes straight to them instead of searching the tree.

use std::collections::HashMap;

use serde_json::Value;

/// One element the model may act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// The qualified ref agent-desktop needs: `@s8f3k2p9:e12`.
    pub full_ref: String,
    pub role: String,
    pub name: String,
    /// The element is already selected (a list row, a mailbox).
    pub selected: bool,
}

#[derive(Debug, Clone, Default)]
pub struct View {
    pub text: String,
    /// Short ref (`e12`, `m330`) → element.
    pub refs: HashMap<String, Element>,
    /// Some lines didn't fit and were left out.
    pub truncated: bool,
}

const MAX_VALUE: usize = 120;
const MAX_DEPTH_INDENT: usize = 6;
/// States worth a model's attention. "focused" is left out: Mail marks every row of a list focused.
const STATES_SHOWN: &[&str] = &["selected", "checked", "disabled", "expanded", "offscreen"];

/// A node already turned into its line, with the size of everything below it.
struct Rendered {
    line: Option<String>,
    entry: Option<(String, Element)>,
    children: Vec<Rendered>,
    size: usize,
}

/// Compact a window `snapshot` result (its `data`) into at most `limit` characters.
pub fn compact(data: &Value, limit: usize) -> View {
    let mut view = View::default();
    let mut lines = Vec::new();
    let mut budget = limit;
    if let Some(title) = data["window"]["title"].as_str().map(plain).filter(|t| !t.is_empty()) {
        let line = format!("window \"{}\"", clip(&title));
        budget = budget.saturating_sub(line.len() + 1);
        lines.push(line);
    }
    let tree = build(&data["tree"], 0, &[], "e");
    let used = emit(&tree, budget, &mut lines, &mut view.refs);
    view.truncated = used < tree.size;
    if view.truncated {
        lines.push("… (more elements not shown)".into());
    }
    let places = key_places(&view.refs);
    view.text = if places.is_empty() { lines.join("\n") } else { format!("KEY PLACES:\n{}\n\n{}", places.join("\n"), lines.join("\n")) };
    view
}

/// Roles a user types into.
const EDITABLE: &[&str] = &["textfield", "textarea", "searchfield", "textview", "combobox"];
/// Words in a search field's name or placeholder.
const SEARCH_WORDS: &[&str] = &["search", "rechercher", "recherche", "find", "filter", "filtrer", "buscar", "suche", "cerca"];
/// Words in a message box's name or placeholder ("Type a message", "Saisissez un message", "Reply…").
const MESSAGE_WORDS: &[&str] = &[
    "message",
    "messages",
    "type",
    "write",
    "reply",
    "compose",
    "chat",
    "saisissez",
    "saisir",
    "ecrire",
    "ecrivez",
    "tapez",
    "repondre",
    "escribe",
    "mensaje",
    "nachricht",
    "scrivi",
    "messaggio",
];
/// Words naming a send button.
const SEND_WORDS: &[&str] = &["send", "envoyer", "enviar", "senden", "invia"];

/// The places every chat-like window has, found among the elements shown: the message box, the search
/// field and the send button. A window with one field besides search has it as its message box; a form
/// with several fields gets no guess.
fn key_places(refs: &HashMap<String, Element>) -> Vec<String> {
    let mut shown: Vec<(&String, &Element)> = refs.iter().collect();
    shown.sort_by_key(|(r, _)| (r.chars().next(), r[1..].parse::<u64>().unwrap_or(u64::MAX)));
    let words = |e: &Element| -> Vec<String> { alfred_core::catalog::fold(&e.name).split_whitespace().map(str::to_string).collect() };
    let says = |e: &Element, list: &[&str]| words(e).iter().any(|w| list.contains(&w.as_str()));
    let role = |e: &Element| e.role.to_lowercase();
    let editable: Vec<&(&String, &Element)> = shown.iter().filter(|(_, e)| EDITABLE.contains(&role(e).as_str())).collect();
    let search = editable.iter().find(|(_, e)| role(e) == "searchfield" || says(e, SEARCH_WORDS)).map(|p| **p);
    let others: Vec<&&(&String, &Element)> = editable.iter().filter(|p| search.is_none_or(|s| s.0 != p.0)).collect();
    let message = others.iter().find(|(_, e)| says(e, MESSAGE_WORDS)).map(|p| ***p).or_else(|| (others.len() == 1).then(|| **others[0]));
    let send = shown.iter().find(|(_, e)| role(e) == "button" && says(e, SEND_WORDS)).copied();
    let line = |label: &str, (r, e): (&String, &Element), note: &str| {
        let name = if e.name.is_empty() { String::new() } else { format!(" \"{}\"", clip(&e.name)) };
        format!(" {label}: {r} {}{name}{note}", e.role)
    };
    let mut out = Vec::new();
    out.extend(message.map(|m| line("message box", m, " (to write, type into it)")));
    out.extend(search.map(|s| line("search", s, "")));
    out.extend(send.map(|s| line("send button", s, " (it sends; Alfred asks the user first)")));
    out
}

/// Text without the invisible marks some apps put around names (WhatsApp's left-to-right marks), which
/// would hide a name from matching and from the model.
fn plain(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'))
        .collect::<String>()
        .trim()
        .to_string()
}

fn build(node: &Value, depth: usize, ancestors: &[String], prefix: &str) -> Rendered {
    let role = node["role"].as_str().unwrap_or("element");
    // An unnamed row borrows the name of its first named actionable child (a message row, its sender's
    // cell), so the row's line says what it is and carries its [selected] state where the model looks.
    let own = node["name"].as_str().map(plain).unwrap_or_default();
    let name_owned = if !own.is_empty() {
        own
    } else if node["ref_id"].is_string() {
        plain(borrowed_name(node))
    } else {
        String::new()
    };
    let name = name_owned.as_str();
    let value_owned = node["value"].as_str().map(plain).unwrap_or_default();
    let value = value_owned.as_str();
    let indent = " ".repeat(depth.min(MAX_DEPTH_INDENT));
    let parent = ancestors.last().map(|a| bare(a)).unwrap_or("");
    let mut line = None;
    let mut entry = None;
    if let Some(full) = node["ref_id"].as_str() {
        let short = format!("{prefix}{}", full.rsplit(':').next().unwrap_or(full).trim_start_matches('e'));
        // A cell repeating its row's or item's name adds nothing: its parent is the one to act on.
        let duplicate = !name.is_empty() && name == parent && node_has_ref_parent(ancestors);
        if !duplicate {
            let mut l = format!("{indent}{short} {role}");
            if !name.is_empty() {
                l.push_str(&format!(" \"{}\"", clip(name)));
            }
            if !value.is_empty() && value != name {
                l.push_str(&format!(" = \"{}\"", clip(value)));
            }
            let states: Vec<&str> = node["states"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).filter(|s| STATES_SHOWN.contains(s)).collect())
                .unwrap_or_default();
            if !states.is_empty() {
                l.push_str(&format!(" [{}]", states.join(", ")));
            }
            line = Some(l);
        }
        let selected = node["states"].as_array().is_some_and(|a| a.iter().any(|v| v == "selected"));
        entry = Some((short, Element { full_ref: full.to_string(), role: role.to_string(), name: name.to_string(), selected }));
    } else {
        // No ref: only content worth reading gets a line, and not when it repeats a label just above.
        let content = if !value.is_empty() { value } else { name };
        if !content.is_empty() && depth > 0 && !ancestors.iter().rev().take(2).any(|a| bare(a) == content) {
            line = Some(format!("{indent}text \"{}\"", clip(content)));
        }
    }
    let mut next: Vec<String> = ancestors.to_vec();
    next.push(if node["ref_id"].is_string() { format!("\u{1}{name}") } else { name.to_string() });
    let children: Vec<Rendered> = node["children"]
        .as_array()
        .map(|cs| cs.iter().filter(|c| c.is_object()).map(|c| build(c, depth + 1, &next, prefix)).collect())
        .unwrap_or_default();
    let size = line.as_ref().map_or(0, |l| l.len() + 1) + children.iter().map(|c| c.size).sum::<usize>();
    Rendered { line, entry, children, size }
}

fn borrowed_name(node: &Value) -> &str {
    node["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["ref_id"].is_string())
        .find_map(|c| c["name"].as_str().map(str::trim).filter(|n| !n.is_empty()))
        .unwrap_or("")
}

/// Ancestors are kept as names; one with a ref is marked with a leading \u{1} so the duplicate check can tell.
fn bare(a: &str) -> &str {
    a.trim_start_matches('\u{1}')
}

fn node_has_ref_parent(ancestors: &[String]) -> bool {
    ancestors.last().is_some_and(|a| a.starts_with('\u{1}'))
}

/// Write `r` into `lines` within `budget` characters; returns the characters used.
fn emit(r: &Rendered, budget: usize, lines: &mut Vec<String>, refs: &mut HashMap<String, Element>) -> usize {
    let mut used = 0;
    if let Some(l) = &r.line {
        if l.len() + 1 > budget {
            return 0;
        }
        lines.push(l.clone());
        used += l.len() + 1;
    }
    if let (Some((short, el)), Some(_)) = (&r.entry, &r.line) {
        refs.insert(short.clone(), el.clone());
    }
    let sizes: Vec<usize> = r.children.iter().map(|c| c.size).collect();
    // Where a big region sits beside others (a sidebar next to a message list), the budget is shared
    // fairly, small branches first. Where every branch is small (the rows of a list, the labels of a row),
    // they are kept whole and in order: the first rows complete, the rest cut.
    let regions = sizes.len() >= 2 && sizes.iter().any(|&s| s >= REGION);
    let allocations = if regions { share(&sizes, budget - used) } else { in_order(&sizes, budget - used) };
    for (child, alloc) in r.children.iter().zip(allocations) {
        used += emit(child, alloc, lines, refs);
    }
    used
}

/// A branch at least this big (in characters) is a region of the window.
const REGION: usize = 1200;

/// Each branch in turn gets what it needs while the budget lasts.
fn in_order(sizes: &[usize], budget: usize) -> Vec<usize> {
    let mut left = budget;
    sizes
        .iter()
        .map(|&s| {
            let a = s.min(left);
            left -= a;
            a
        })
        .collect()
}

/// Water-filling: each branch gets an equal part of what's left, and small branches pass on what they
/// don't need.
fn share(sizes: &[usize], budget: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| sizes[i]);
    let mut out = vec![0; sizes.len()];
    let mut left = budget;
    for (k, &i) in order.iter().enumerate() {
        let fair = left / (sizes.len() - k);
        out[i] = sizes[i].min(fair);
        left -= out[i];
    }
    out
}

/// The app's menus as lines (`m330 "Répondre"` under `m327 menu "Message"`): top-level menus and the
/// commands directly inside them. The Apple menu (shut down, restart, log out) and the Services submenu are
/// left out: no app task needs them. Disabled items and separators are dropped.
pub fn compact_menubar(data: &Value, limit: usize) -> View {
    let mut view = View::default();
    let mut lines = Vec::new();
    let mut used = 0;
    let tops = data["tree"]["children"].as_array().cloned().unwrap_or_default();
    'menus: for top in tops.iter().skip_while(|t| t["name"].as_str() == Some("Apple")) {
        let Some((short, el)) = menu_entry(top) else { continue };
        let head = format!("{short} menu \"{}\"", clip(&el.name));
        let items: Vec<(String, Element)> = top["children"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|menu| menu["children"].as_array().cloned().unwrap_or_default())
            .filter_map(|item| menu_entry(&item))
            .filter(|(_, e)| e.name != "Services")
            .collect();
        if used + head.len() + 1 > limit {
            view.truncated = true;
            break;
        }
        used += head.len() + 1;
        lines.push(head);
        view.refs.insert(short, el);
        for (s, e) in items {
            let line = format!(" {s} \"{}\"", clip(&e.name));
            if used + line.len() + 1 > limit {
                view.truncated = true;
                break 'menus;
            }
            used += line.len() + 1;
            lines.push(line);
            view.refs.insert(s, e);
        }
    }
    view.text = lines.join("\n");
    view
}

fn menu_entry(item: &Value) -> Option<(String, Element)> {
    let full = item["ref_id"].as_str()?;
    let name = item["name"].as_str().unwrap_or("").trim();
    let disabled = item["states"].as_array().is_some_and(|s| s.iter().any(|v| v == "disabled"));
    if name.is_empty() || disabled {
        return None;
    }
    let short = format!("m{}", full.rsplit(':').next().unwrap_or(full).trim_start_matches('e'));
    Some((short, Element { full_ref: full.to_string(), role: "menuitem".into(), name: name.to_string(), selected: false }))
}

/// The window and the menu bar in one view, each under its heading.
pub fn merge(window: View, menubar: View) -> View {
    if menubar.text.is_empty() {
        return window;
    }
    let mut refs = window.refs;
    refs.extend(menubar.refs);
    View {
        text: format!("{}\n\nMENU BAR (commands; they act on what is selected in the window):\n{}", window.text, menubar.text),
        refs,
        truncated: window.truncated || menubar.truncated,
    }
}

fn clip(s: &str) -> String {
    let s = s.replace(['\n', '\r', '\t'], " ").replace('"', "'");
    let s = s.trim();
    if s.chars().count() <= MAX_VALUE { s.to_string() } else { s.chars().take(MAX_VALUE - 1).collect::<String>() + "…" }
}

/// The short ref a model meant: "e12", "@s1:e12", "12" and "E12" all mean "e12"; "m330" stays a menu ref.
pub fn short_ref(raw: &str) -> String {
    let r = raw.trim().rsplit(':').next().unwrap_or(raw).trim().trim_start_matches('@').to_lowercase();
    if r.chars().all(|c| c.is_ascii_digit()) && !r.is_empty() { format!("e{r}") } else { r }
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
