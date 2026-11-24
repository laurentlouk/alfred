use super::*;

fn act(text: &str) -> Action {
    match route(text) {
        Some(Route::Act(a)) => a,
        other => panic!("{text:?} should route to an action, got {other:?}"),
    }
}

fn app(name: &str) -> Action {
    Action::OpenApp { name: name.into() }
}

fn search_for(q: &str) -> Action {
    Action::WebSearch { query: q.into(), browser: None }
}

fn note_of(title: &str, body: &str) -> Action {
    Action::CreateNote { title: title.into(), body: body.into() }
}

#[test]
fn opens_apps() {
    assert_eq!(act("open notes"), app("Notes"));
    assert_eq!(act("Open Notes."), app("Notes"));
    assert_eq!(act("hey Alfred, could you open the notes app for me?"), app("Notes"));
    assert_eq!(act("launch chrome"), app("Google Chrome"));
    assert_eq!(act("start spotify"), app("Spotify"));
    assert_eq!(act("open up activity monitor"), app("Activity Monitor"));
    assert_eq!(act("switch to Safari"), app("Safari"));
    assert_eq!(act("please open system settings"), app("System Settings"));
}

#[test]
fn opens_links_and_known_sites() {
    let link = |u: &str| Action::OpenUrl { url: u.into(), browser: None };
    assert_eq!(act("open youtube"), link("https://www.youtube.com"));
    assert_eq!(act("go to github.com"), link("https://github.com"));
    assert_eq!(act("visit https://docs.rs/rune"), link("https://docs.rs/rune"));
    assert_eq!(act("open example.com in safari"), Action::OpenUrl { url: "https://example.com".into(), browser: Some("Safari".into()) });
}

#[test]
fn quits_apps_but_not_windows() {
    assert_eq!(act("quit safari"), Action::QuitApp { name: "Safari".into() });
    assert_eq!(act("close Music"), Action::QuitApp { name: "Music".into() });
    assert_eq!(route("close the window"), None);
    assert_eq!(route("close this tab"), None);
}

#[test]
fn saves_notes_in_many_phrasings() {
    assert_eq!(act("take a note: buy milk"), note_of("Buy milk", "Buy milk"));
    assert_eq!(act("save a note that I need to buy milk tomorrow"), note_of("I need to buy milk tomorrow", "I need to buy milk tomorrow"));
    assert_eq!(act("make a note of the wifi password hint"), note_of("The wifi password hint", "The wifi password hint"));
    assert_eq!(act("note down call the bank"), note_of("Call the bank", "Call the bank"));
    assert_eq!(act("jot down eggs, flour and butter"), note_of("Eggs, flour and butter", "Eggs, flour and butter"));
    assert_eq!(act("remember that I parked on level 3"), note_of("I parked on level 3", "I parked on level 3"));
    assert_eq!(act("save oat milk in my notes"), note_of("Oat milk", "Oat milk"));
    assert_eq!(act("add lemons to notes"), note_of("Lemons", "Lemons"));
}

#[test]
fn the_owners_example_open_notes_and_save() {
    assert_eq!(act("open Notes and save buy milk and bread"), note_of("Buy milk and bread", "Buy milk and bread"));
    assert_eq!(act("open notes and write meeting moved to 3pm"), note_of("Meeting moved to 3pm", "Meeting moved to 3pm"));
    // Nothing to save yet: open Notes and ask what to write.
    assert_eq!(
        route("open Notes and save something for me"),
        Some(Route::Ask { question: "What would you like me to write down?".into(), slot: Slot::NoteBody, first: Some(app("Notes")) })
    );
}

#[test]
fn titled_notes_split_title_and_body() {
    assert_eq!(act("create a note called Groceries with milk and eggs"), note_of("Groceries", "Milk and eggs"));
    assert_eq!(act("new note titled Ideas: a launcher for notes"), note_of("Ideas", "A launcher for notes"));
}

#[test]
fn reminders_with_and_without_delays() {
    let r = |t: &str, m: Option<u32>| Action::CreateReminder { title: t.into(), minutes: m };
    assert_eq!(act("remind me to call mum"), r("Call mum", None));
    assert_eq!(act("remind me in 10 minutes to stretch"), r("Stretch", Some(10)));
    assert_eq!(act("remind me to take the bins out in 2 hours"), r("Take the bins out", Some(120)));
    assert_eq!(act("set a reminder to water the plants in half an hour"), r("Water the plants", Some(30)));
    assert_eq!(act("remind me to stretch in an hour"), r("Stretch", Some(60)));
    assert_eq!(act("remind me in five minutes to check the oven"), r("Check the oven", Some(5)));
    assert_eq!(act("remember to buy stamps"), r("Buy stamps", None));
    assert_eq!(act("remind me to buy milk in the morning"), r("Buy milk in the morning", None));
}

#[test]
fn web_searches() {
    assert_eq!(act("search google for rust rune ui"), search_for("rust rune ui"));
    assert_eq!(act("google best ramen in Lisbon"), search_for("best ramen in Lisbon"));
    assert_eq!(act("look up the weather in Paris"), search_for("the weather in Paris"));
    assert_eq!(act("search for kokoro tts online"), search_for("kokoro tts"));
    assert_eq!(act("open google and search for cats"), search_for("cats"));
    assert_eq!(
        act("open safari and search for the rune ui framework"),
        Action::WebSearch { query: "the rune ui framework".into(), browser: Some("Safari".into()) }
    );
    assert_eq!(
        act("search ollama models in chrome"),
        Action::WebSearch { query: "ollama models".into(), browser: Some("Google Chrome".into()) }
    );
}

#[test]
fn time_questions() {
    for q in ["what time is it?", "What's the time", "what's the date", "what day is it", "Alfred, what time is it"] {
        assert_eq!(act(q), Action::GetTime, "{q}");
    }
    assert_eq!(route("time to go home"), None);
    assert_eq!(route("what time does the bakery close"), None);
}

#[test]
fn missing_content_asks_instead_of_guessing() {
    assert!(matches!(route("take a note"), Some(Route::Ask { slot: Slot::NoteBody, first: None, .. })));
    assert!(matches!(route("remind me"), Some(Route::Ask { slot: Slot::ReminderTitle, .. })));
    assert!(matches!(route("search google"), Some(Route::Ask { slot: Slot::SearchQuery { browser: None }, .. })));
}

#[test]
fn slots_are_filled_by_the_next_answer() {
    assert_eq!(Slot::NoteBody.fill("buy milk"), Some(note_of("Buy milk", "Buy milk")));
    assert_eq!(
        Slot::ReminderTitle.fill("call mum in 5 minutes"),
        Some(Action::CreateReminder { title: "Call mum".into(), minutes: Some(5) })
    );
    assert_eq!(Slot::SearchQuery { browser: None }.fill("rune"), Some(search_for("rune")));
    assert_eq!(Slot::NoteBody.fill("never mind"), None);
    assert_eq!(Slot::NoteBody.fill(""), None);
}

#[test]
fn leaves_everything_else_to_the_model() {
    for q in [
        "hello",
        "how are you",
        "could you jot it down that the plumber comes at 3",
        "what's the capital of France",
        "tell me a joke",
        "open the pod bay doors and sing",
        "start a timer for five minutes",
        "go to settings",
        "",
        "   ",
    ] {
        if let Some(r) = route(q) {
            // "jot it down …" is a note; everything else must fall through.
            assert!(q.contains("jot"), "{q:?} should go to the model, got {r:?}");
        }
    }
}

#[test]
fn clean_strips_politeness_only_at_the_edges() {
    assert_eq!(clean("Hey Alfred, please open Notes for me, thanks!"), "open Notes");
    assert_eq!(clean("Could you please search for thank you cards"), "search for thank you cards");
    assert_eq!(clean("alfredo pasta recipe"), "alfredo pasta recipe");
}
