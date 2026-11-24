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
//! snapshot prefix and mapped back when the model picks one.

use std::collections::HashMap;

use serde_json::Value;

/// One element the model may act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// The qualified ref agent-desktop needs: `@s8f3k2p9:e12`.
    pub full_ref: String,
    pub role: String,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct View {
    pub text: String,
    /// Short ref (`e12`) → element.
    pub refs: HashMap<String, Element>,
    /// Some lines didn't fit and were left out.
    pub truncated: bool,
}

const MAX_VALUE: usize = 120;
const MAX_DEPTH_INDENT: usize = 6;

/// Compact a `snapshot` result (its `data`) into at most `limit` characters.
pub fn compact(data: &Value, limit: usize) -> View {
    let mut view = View::default();
    let mut lines = Vec::new();
    if let Some(title) = data["window"]["title"].as_str().filter(|t| !t.is_empty()) {
        lines.push(format!("window \"{}\"", clip(title)));
    }
    walk(&data["tree"], 0, &mut lines, &mut view.refs);
    let mut used = 0;
    let mut kept = Vec::new();
    for line in lines {
        if used + line.len() + 1 > limit {
            view.truncated = true;
            break;
        }
        used += line.len() + 1;
        kept.push(line);
    }
    if view.truncated {
        kept.push("… (more elements not shown)".into());
    }
    view.text = kept.join("\n");
    view
}

fn walk(node: &Value, depth: usize, lines: &mut Vec<String>, refs: &mut HashMap<String, Element>) {
    if !node.is_object() {
        return;
    }
    let role = node["role"].as_str().unwrap_or("element");
    let name = node["name"].as_str().unwrap_or("").trim();
    let value = node["value"].as_str().map(str::trim).unwrap_or("");
    let indent = " ".repeat(depth.min(MAX_DEPTH_INDENT));
    if let Some(full) = node["ref_id"].as_str() {
        let short = full.rsplit(':').next().unwrap_or(full).to_string();
        let mut line = format!("{indent}{short} {role}");
        if !name.is_empty() {
            line.push_str(&format!(" \"{}\"", clip(name)));
        }
        if !value.is_empty() && value != name {
            line.push_str(&format!(" = \"{}\"", clip(value)));
        }
        let states: Vec<&str> = node["states"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| ["selected", "focused", "checked", "disabled", "expanded"].contains(s))
                    .collect()
            })
            .unwrap_or_default();
        if !states.is_empty() {
            line.push_str(&format!(" [{}]", states.join(", ")));
        }
        lines.push(line);
        refs.insert(short, Element { full_ref: full.to_string(), role: role.to_string(), name: name.to_string() });
    } else {
        // No ref: only content worth reading gets a line; structure is just walked through.
        let content = if !value.is_empty() { value } else { name };
        if !content.is_empty() && depth > 0 {
            lines.push(format!("{indent}text \"{}\"", clip(content)));
        }
    }
    if let Some(children) = node["children"].as_array() {
        for c in children {
            walk(c, depth + 1, lines, refs);
        }
    }
}

fn clip(s: &str) -> String {
    let s = s.replace(['\n', '\r', '\t'], " ").replace('"', "'");
    if s.chars().count() <= MAX_VALUE { s } else { s.chars().take(MAX_VALUE - 1).collect::<String>() + "…" }
}

/// The short ref a model meant: "e12", "@s1:e12", "12" and "E12" all mean "e12".
pub fn short_ref(raw: &str) -> String {
    let r = raw.trim().rsplit(':').next().unwrap_or(raw).trim().trim_start_matches('@').to_lowercase();
    if r.chars().all(|c| c.is_ascii_digit()) && !r.is_empty() { format!("e{r}") } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> Value {
        json!({
            "app": "Mail", "snapshot_id": "s1", "window": {"id": "w1", "title": "Inbox – 2 messages"},
            "tree": {"role": "window", "name": "Inbox", "children": [
                {"role": "toolbar", "children": [
                    {"ref_id": "@s1:e1", "role": "button", "name": "Reply"},
                    {"ref_id": "@s1:e2", "role": "button", "name": "Delete"}
                ]},
                {"role": "table", "name": "Messages", "children": [
                    {"ref_id": "@s1:e3", "role": "row", "name": "Marie Curie, Dinner on Friday?", "states": ["selected", "enabled"]}
                ]},
                {"role": "statictext", "value": "Hi!\nAre you free for dinner on \"Friday\"?"},
                {"role": "group"}
            ]}
        })
    }

    #[test]
    fn compacts_to_ref_lines_and_text_lines() {
        let v = compact(&snapshot(), 9000);
        assert_eq!(
            v.text,
            "window \"Inbox – 2 messages\"\n  e1 button \"Reply\"\n  e2 button \"Delete\"\n text \"Messages\"\n  e3 row \"Marie Curie, Dinner on Friday?\" [selected]\n text \"Hi! Are you free for dinner on 'Friday'?\""
        );
        assert_eq!(v.refs["e1"], Element { full_ref: "@s1:e1".into(), role: "button".into(), name: "Reply".into() });
        assert!(!v.truncated);
    }

    #[test]
    fn long_views_are_cut_and_say_so() {
        let v = compact(&snapshot(), 60);
        assert!(v.truncated);
        assert!(v.text.ends_with("(more elements not shown)"));
        assert!(v.text.len() < 120);
    }

    #[test]
    fn refs_are_read_loosely() {
        for r in ["e12", "@s1:e12", "12", "E12", " e12 "] {
            assert_eq!(short_ref(r), "e12", "{r}");
        }
    }
}
