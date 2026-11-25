use super::super::*;
use crate::action::MusicCommand;

fn with_claude() -> Catalog {
    Catalog { claude: true, ..Catalog::default() }
}

fn act_in(text: &str, catalog: &Catalog) -> Action {
    match route_with(text, catalog) {
        Some(Route::Act(a)) => a,
        other => panic!("{text:?} should route to an action, got {other:?}"),
    }
}

fn act(text: &str) -> Action {
    act_in(text, &Catalog::default())
}

fn music(command: MusicCommand, what: Option<&str>) -> Action {
    Action::Music { command, what: what.map(str::to_string) }
}

fn video(q: &str) -> Action {
    Action::FindVideo { query: q.into() }
}

fn code(prompt: &str, folder: Option<&str>) -> Action {
    Action::ClaudeCode { prompt: prompt.into(), folder: folder.map(str::to_string) }
}

fn text_to(to: &str, text: &str) -> Action {
    Action::SendMessage { app: "Messages".into(), to: Some(to.into()), text: text.into() }
}

fn mail(query: Option<&str>, from: Option<&str>) -> Action {
    Action::SearchMail { query: query.map(str::to_string), from: from.map(str::to_string) }
}

#[test]
fn music_is_played_paused_and_skipped_without_the_model() {
    use MusicCommand::*;
    for (said, want) in [
        ("pause the music", Pause),
        ("pause", Pause),
        ("stop the music", Pause),
        ("Mets en pause.", Pause),
        ("resume the music", Play),
        ("play some music", Play),
        ("play music", Play),
        ("mets de la musique", Play),
        ("next song", Next),
        ("skip this song", Next),
        ("skip", Next),
        ("skip to the next one", Next),
        ("who is this singer?", NowPlaying),
        ("chanson suivante", Next),
        ("previous track", Previous),
        ("go back a song", Previous),
        ("what's playing?", NowPlaying),
        ("what song is this", NowPlaying),
        ("what am I listening to", NowPlaying),
        ("what playlists do I have?", Playlists),
        ("list my playlists", Playlists),
    ] {
        assert_eq!(act(said), music(want, None), "{said}");
    }
}

#[test]
fn play_names_what_to_play() {
    use MusicCommand::Play;
    assert_eq!(act("play my running playlist"), music(Play, Some("running")));
    assert_eq!(act("play the playlist Focus"), music(Play, Some("Focus")));
    assert_eq!(act("play Daft Punk"), music(Play, Some("Daft Punk")));
    assert_eq!(act("play songs by Daft Punk"), music(Play, Some("Daft Punk")));
    assert_eq!(act("put on some jazz"), music(Play, Some("jazz")));
    assert_eq!(act("listen to Discovery by Daft Punk"), music(Play, Some("Discovery by Daft Punk")));
    assert_eq!(act("play the album Random Access Memories on Apple Music"), music(Play, Some("Random Access Memories")));
    assert_eq!(act("joue ma playlist sport"), music(Play, Some("sport")));
}

#[test]
fn play_leaves_pages_videos_and_other_apps_alone() {
    for said in ["play it", "play the second video", "play the video", "play lofi on spotify", "mets un minuteur de 5 minutes"] {
        assert!(!matches!(route(said), Some(Route::Act(Action::Music { .. }))), "{said}: {:?}", route(said));
    }
}

#[test]
fn videos_are_found_on_youtube() {
    assert_eq!(act("find a video explaining how transformers work"), video("how transformers work"));
    assert_eq!(act("find me a video about black holes"), video("black holes"));
    assert_eq!(act("show me a video on how to tie a tie"), video("how to tie a tie"));
    assert_eq!(act("search youtube for rust lifetimes"), video("rust lifetimes"));
    assert_eq!(act("youtube rust lifetimes"), video("rust lifetimes"));
    assert_eq!(act("on YouTube, find an explanation of quantum entanglement"), video("quantum entanglement"));
    assert_eq!(act("find explanations about quantum entanglement on youtube"), video("quantum entanglement"));
    assert_eq!(act("play lofi beats on youtube"), video("lofi beats"));
    assert_eq!(act("open youtube and find a video about sourdough"), video("sourdough"));
    assert_eq!(act("trouve une vidéo sur les trous noirs"), video("les trous noirs"));
    // Opening the site stays opening the site.
    assert_eq!(act("open youtube"), Action::OpenUrl { url: "https://www.youtube.com".into(), browser: None });
    assert_eq!(route("find a video"), None, "nothing to look for");
}

#[test]
fn claude_code_gets_the_prompt_and_the_folder_said() {
    let c = with_claude();
    assert_eq!(act_in("ask claude code to fix the failing tests in the alfred project", &c), code("Fix the failing tests", Some("alfred")));
    assert_eq!(act_in("tell Claude Code to add a dark mode", &c), code("Add a dark mode", None));
    assert_eq!(act_in("open claude code and ask it to review my last commit", &c), code("Review my last commit", None));
    assert_eq!(
        act_in("start a new Claude Code session to explain this repo in the motion-video folder", &c),
        code("Explain this repo", Some("motion-video"))
    );
    assert_eq!(act_in("claude code: write tests for the parser", &c), code("Write tests for the parser", None));
    assert_eq!(act_in("open claude code", &c), code("", None), "an empty session");
    assert_eq!(act_in("open claude code in my alfred repo", &c), code("", Some("alfred")));
    // Coding work asked of "Claude" is Claude Code too; anything else is a chat.
    assert_eq!(act_in("ask claude to fix the bug in the parser", &c), code("Fix the bug in the parser", None));
    assert_eq!(
        act_in("ask Claude to summarize today's AI news", &c),
        Action::SendMessage { app: "Claude".into(), to: None, text: "Summarize today's AI news".into() }
    );
    // Without the Claude app or Claude Code, the model decides.
    assert!(!matches!(route("ask claude code to fix the tests"), Some(Route::Act(Action::ClaudeCode { .. }))));
}

#[test]
fn texts_go_through_messages_with_the_words_as_said() {
    assert_eq!(act("text Ada that I'm running late"), text_to("Ada", "I'm running late"));
    assert_eq!(act("send a text to Ada Lovelace saying see you at 7"), text_to("Ada Lovelace", "See you at 7"));
    assert_eq!(act("Send an SMS to mum: on my way!"), text_to("mum", "On my way!"));
    assert_eq!(act("text +33 6 12 34 56 78 saying hello"), text_to("+33 6 12 34 56 78", "Hello"));
    assert_eq!(act("envoie un SMS à Marie pour lui dire que j'arrive"), text_to("Marie", "J'arrive"));
    // No message, or another app named: the model decides.
    assert_eq!(route("text Ada"), None);
    assert!(!matches!(route("text Ada on WhatsApp saying hi"), Some(Route::Act(Action::SendMessage { .. }))));
}

#[test]
fn mail_is_searched_by_subject_and_sender() {
    assert_eq!(act("find emails about the invoice"), mail(Some("invoice"), None));
    assert_eq!(act("search my mail for taxes"), mail(Some("taxes"), None));
    assert_eq!(act("find the emails from Ada about lunch"), mail(Some("lunch"), Some("Ada")));
    assert_eq!(act("do I have any emails from Free?"), mail(None, Some("Free")));
    assert_eq!(act("any email about the plumber?"), mail(Some("plumber"), None));
    assert_eq!(act("cherche les mails de Marie"), mail(None, Some("Marie")));
    // Reading the newest one is still reading.
    assert_eq!(act("read my last email"), Action::ReadMail { from: None });
    assert_eq!(route("find emails"), None);
}
