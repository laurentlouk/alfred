use serde_json::json;

use super::*;

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
    assert_eq!(v.refs["e1"], Element { full_ref: "@s1:e1".into(), role: "button".into(), name: "Reply".into(), selected: false });
    assert!(v.refs["e3"].selected);
    assert!(!v.truncated);
}

#[test]
fn long_views_are_cut_and_say_so() {
    let v = compact(&snapshot(), 60);
    assert!(v.truncated);
    assert!(v.text.ends_with("(more elements not shown)"));
    assert!(v.text.len() < 140);
}

#[test]
fn a_big_sidebar_cannot_crowd_out_the_list() {
    let folders: Vec<Value> =
        (0..200).map(|i| json!({"ref_id": format!("@s:e{}", i + 10), "role": "treeitem", "name": format!("Folder number {i}")})).collect();
    let rows: Vec<Value> =
        (0..5).map(|i| json!({"ref_id": format!("@s:e{}", i + 500), "role": "row", "name": format!("Message {i}")})).collect();
    let data = json!({"tree": {"role": "window", "children": [
        {"role": "outline", "name": "Mailboxes", "children": folders},
        {"role": "table", "name": "Messages", "children": rows}
    ]}});
    let v = compact(&data, 1500);
    assert!(v.truncated);
    for i in 0..5 {
        assert!(v.text.contains(&format!("e{} row \"Message {i}\"", i + 500)), "row {i} missing:\n{}", v.text);
    }
    assert!(v.text.len() <= 1500 + 40);
    assert!(!v.refs.contains_key("e209"), "folders past the budget have no ref");
}

#[test]
fn repeated_labels_are_dropped() {
    let data = json!({"tree": {"role": "window", "children": [
        {"ref_id": "@s:e3", "role": "treeitem", "name": "Inbox", "children": [
            {"ref_id": "@s:e4", "role": "cell", "name": "Inbox", "children": [
                {"role": "statictext", "name": "Inbox"},
                {"ref_id": "@s:e5", "role": "button", "name": "12"}
            ]}
        ]},
        {"ref_id": "@s:e7", "role": "row", "children": [
            {"ref_id": "@s:e8", "role": "cell", "name": "Ada", "states": ["offscreen"], "children": [
                {"role": "statictext", "name": "Ada", "value": "Ada"},
                {"role": "statictext", "name": "Lunch on Friday?"}
            ]}
        ]}
    ]}});
    let v = compact(&data, 9000);
    assert_eq!(v.text, " e3 treeitem \"Inbox\"\n   e5 button \"12\"\n e7 row \"Ada\"\n   text \"Lunch on Friday?\"");
    assert!(!v.refs.contains_key("e4"), "the duplicate cell isn't offered");
    assert!(!v.refs.contains_key("e8"), "the row, named after its sender, is what to click");
    assert_eq!(v.refs["e7"].name, "Ada");
}

fn menubar() -> Value {
    let item = |r: &str, name: &str| json!({"ref_id": format!("@m:e{r}"), "role": "menuitem", "name": name});
    json!({"tree": {"role": "menu", "children": [
        {"ref_id": "@m:e1", "role": "menuitem", "name": "Apple", "children": [{"role": "menu", "children": [item("2", "Shut Down…"), item("3", "Restart…")]}]},
        {"ref_id": "@m:e71", "role": "menuitem", "name": "Mail", "children": [{"role": "menu", "children": [
            item("72", "About Mail"), item("79", "Services"), {"ref_id": "@m:e73", "role": "menuitem", "states": ["disabled"]}, item("94", "Quit Mail")
        ]}]},
        {"ref_id": "@m:e327", "role": "menuitem", "name": "Message", "children": [{"role": "menu", "children": [
            item("330", "Reply"),
            {"ref_id": "@m:e348", "role": "menuitem", "name": "Flag", "children": [{"role": "menu", "children": [item("349", "Orange")]}]},
            {"ref_id": "@m:e331", "role": "menuitem", "name": "Reply All", "states": ["disabled"]}
        ]}]}
    ]}})
}

#[test]
fn the_menu_bar_lists_commands_without_the_apple_menu() {
    let v = compact_menubar(&menubar(), 9000);
    assert_eq!(
        v.text,
        "m71 menu \"Mail\"\n m72 \"About Mail\"\n m94 \"Quit Mail\"\nm327 menu \"Message\"\n m330 \"Reply\"\n m348 \"Flag\""
    );
    assert_eq!(v.refs["m330"].full_ref, "@m:e330");
    assert!(!v.refs.contains_key("m2") && !v.refs.contains_key("m349") && !v.refs.contains_key("m331"));
}

#[test]
fn window_and_menu_bar_merge_without_ref_clashes() {
    let v = merge(compact(&snapshot(), 9000), compact_menubar(&menubar(), 9000));
    assert!(v.text.contains("MENU BAR"));
    assert_eq!(v.refs["e3"].role, "row");
    assert_eq!(v.refs["m330"].name, "Reply");
}

#[test]
fn shares_are_fair_and_small_branches_give_back() {
    assert_eq!(share(&[10, 1000, 1000], 610), vec![10, 300, 300]);
    assert_eq!(share(&[5, 5], 100), vec![5, 5]);
    assert_eq!(share(&[], 100), Vec::<usize>::new());
}

#[test]
fn refs_are_read_loosely() {
    for r in ["e12", "@s1:e12", "12", "E12", " e12 "] {
        assert_eq!(short_ref(r), "e12", "{r}");
    }
    assert_eq!(short_ref("m330"), "m330");
}

#[test]
fn rows_stay_whole_and_the_first_ones_come_first() {
    let rows: Vec<Value> = (0..30)
        .map(|i| {
            json!({"ref_id": format!("@s:e{}", 100 + i), "role": "row", "children": [
                {"ref_id": format!("@s:e{}", 200 + i), "role": "cell", "name": format!("Sender {i}")},
                {"role": "statictext", "value": "00:55"},
                {"role": "statictext", "value": format!("Subject number {i} about something")},
                {"role": "statictext", "value": "A preview of the message body that is fairly long, as previews are."}
            ]})
        })
        .collect();
    let data = json!({"tree": {"role": "window", "children": [{"role": "table", "name": "Messages", "children": rows}]}});
    let v = compact(&data, 1200);
    assert!(v.text.contains("Subject number 0 about something") && v.text.contains("Subject number 3 about something"), "{}", v.text);
    assert!(!v.text.contains("Sender 29"), "later rows are cut, not shredded");
}
