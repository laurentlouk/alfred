//! Apple Music through its own scripting: play, pause, skip, say what's playing, list the playlists, and
//! play a playlist, song, artist or album from the library. What isn't in the library opens Music's
//! catalogue search, since a script can't play from Apple Music's catalogue.
//!
//! What to play reaches the script through `on run argv`, never as script source.

use std::process::Command;
use std::time::Duration;

use alfred_core::Outcome;
use alfred_core::action::MusicCommand;

use crate::{osascript, run};

/// Separates an answer's fields; separates the playlists' names.
const SEP: char = '\u{1f}';
const RS: char = '\u{1e}';

/// `argv`: the command (play, pause, next, previous, now, playlists), what to play ("" for nothing), the
/// title and the artist when it was said as "X by Y" ("" otherwise). Pausing, skipping and asking never
/// start Music. Answers: `notrunning`, `notfound`, `paused`, `stopped`, `playlists` then the names,
/// or the player state (`playing`, `paused`), the track, its artist and the playlist that was started.
const MUSIC_SCRIPT: &str = r#"on run argv
    set cmd to item 1 of argv
    set what to item 2 of argv
    set title to item 3 of argv
    set byArtist to item 4 of argv
    set sep to character id 31
    if cmd is not "play" and cmd is not "playlists" then
        if application "Music" is not running then return "notrunning"
    end if
    set startedList to ""
    tell application "Music"
        if cmd is "pause" then
            pause
            return "paused"
        else if cmd is "next" then
            next track
        else if cmd is "previous" then
            previous track
        else if cmd is "playlists" then
            set listNames to name of every user playlist whose special kind is none
            set saved to AppleScript's text item delimiters
            set AppleScript's text item delimiters to character id 30
            set out to "playlists" & sep & (listNames as text)
            set AppleScript's text item delimiters to saved
            return out
        else if cmd is "play" then
            if what is "" then
                play
            else
                set found to my pick(what, title, byArtist)
                if found is missing value then return "notfound"
                if item 1 of found is "playlist" then set startedList to name of (item 2 of found)
                play (item 2 of found)
            end if
        end if
        set stateName to player state as text
        if stateName is "stopped" then return "stopped"
        set t to current track
        set who to ""
        try
            set who to artist of t
        end try
        return stateName & sep & (name of t) & sep & who & sep & startedList
    end tell
end run

-- A playlist of that name, then one whose name has it; then, in the library, a song of exactly that name
-- ("Stand By Me"), the song said with its artist ("Discovery by Daft Punk"), a song, an artist, then an
-- album (from its first track).
on pick(what, title, byArtist)
    tell application "Music"
        try
            return {"playlist", (first user playlist whose name is what)}
        end try
        try
            return {"playlist", (first user playlist whose name contains what)}
        end try
        set lib to library playlist 1
        repeat with h in (search lib for what only names)
            if (name of h) is what then return {"track", contents of h}
        end repeat
        if byArtist is not "" then
            repeat with h in (search lib for title only names)
                if (artist of h) contains byArtist then return {"track", contents of h}
            end repeat
        end if
        set hits to (search lib for what only names)
        if hits is not {} then return {"track", item 1 of hits}
        set hits to (search lib for what only artists)
        if hits is not {} then return {"track", item 1 of hits}
        set hits to (search lib for what only albums)
        if hits is not {} then
            set best to item 1 of hits
            set al to album of best
            repeat with h in hits
                if (album of h) is al and (track number of h) < (track number of best) then set best to contents of h
            end repeat
            return {"track", best}
        end if
    end tell
    return missing value
end pick"#;

/// What the script answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    NotRunning,
    NotFound,
    Paused,
    Stopped,
    Playlists(Vec<String>),
    /// The player's state, the track and its artist, and the playlist just started ("" for none).
    Track {
        playing: bool,
        name: String,
        artist: String,
        playlist: String,
    },
}

/// Do `command` in Music. Something to play that isn't in the library opens Music's search for it.
pub fn run_command(command: MusicCommand, what: Option<&str>, timeout: Duration) -> Result<Outcome, String> {
    let what = what.map(str::trim).filter(|w| !w.is_empty()).unwrap_or("");
    let (title, artist) = split_by(what);
    let cmd = match command {
        MusicCommand::Play => "play",
        MusicCommand::Pause => "pause",
        MusicCommand::Next => "next",
        MusicCommand::Previous => "previous",
        MusicCommand::NowPlaying => "now",
        MusicCommand::Playlists => "playlists",
    };
    let answer = parse(&osascript(MUSIC_SCRIPT, &[cmd, what, title, artist], timeout)?);
    if answer == Answer::NotFound {
        let mut open = Command::new("open");
        open.arg(catalogue_search(what));
        run(open, timeout)?;
    }
    Ok(Outcome::new(summary(command, what, &answer)))
}

/// "Discovery by Daft Punk" → ("Discovery", "Daft Punk"); anything else → (itself, "").
pub fn split_by(what: &str) -> (&str, &str) {
    for sep in [" by ", " par ", " de "] {
        if let Some(at) = what.to_lowercase().rfind(sep)
            && let (Some(title), Some(artist)) = (what.get(..at), what.get(at + sep.len()..))
            && !title.trim().is_empty()
            && !artist.trim().is_empty()
        {
            return (title.trim(), artist.trim());
        }
    }
    (what, "")
}

/// Music's own search for `what`, in its catalogue.
pub fn catalogue_search(what: &str) -> String {
    format!("music://music.apple.com/search?term={}", crate::claude::percent_encode(what))
}

pub fn parse(out: &str) -> Answer {
    let out = out.trim_end_matches(['\n', '\r']);
    let mut fields = out.split(SEP);
    match fields.next().unwrap_or("").trim() {
        "notrunning" => Answer::NotRunning,
        "notfound" => Answer::NotFound,
        "paused" if !out.contains(SEP) => Answer::Paused,
        "stopped" | "" => Answer::Stopped,
        "playlists" => {
            Answer::Playlists(fields.next().unwrap_or("").split(RS).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect())
        }
        state => {
            let mut next = || fields.next().unwrap_or("").trim().to_string();
            Answer::Track { playing: state == "playing", name: next(), artist: next(), playlist: next() }
        }
    }
}

/// What Alfred says.
pub fn summary(command: MusicCommand, what: &str, answer: &Answer) -> String {
    match answer {
        Answer::NotRunning | Answer::Stopped if command == MusicCommand::Pause => "Nothing is playing.".into(),
        Answer::NotRunning | Answer::Stopped => "Nothing is playing in Music.".into(),
        Answer::NotFound => format!("I didn't find “{what}” in your library, so I've opened Apple Music's search for it."),
        Answer::Paused => "The music is paused.".into(),
        Answer::Playlists(names) if names.is_empty() => "You have no playlists in Music.".into(),
        Answer::Playlists(names) => {
            let shown: Vec<&str> = names.iter().take(12).map(String::as_str).collect();
            let more = names.len() - shown.len();
            let list = if more > 0 { format!("{}, and {more} more", shown.join(", ")) } else { and_list(&shown) };
            match names.len() {
                1 => format!("You have one playlist: {list}."),
                n => format!("You have {n} playlists: {list}."),
            }
        }
        Answer::Track { playing, name, artist, playlist } => {
            let song = if artist.is_empty() { format!("“{name}”") } else { format!("“{name}” by {artist}") };
            match command {
                MusicCommand::Play if !playlist.is_empty() => format!("Playing your playlist “{playlist}”, starting with {song}."),
                MusicCommand::Play => format!("Playing {song}."),
                MusicCommand::Next | MusicCommand::Previous => format!("Now playing {song}."),
                _ if *playing => format!("This is {song}."),
                _ => format!("{} is paused.", capitalized(&song)),
            }
        }
    }
}

/// "A, B and C".
fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_takes_text_only_through_argv() {
        // A constant script (AppleScript's `{}` is an empty list, not a format hole) that runs no shell.
        assert!(MUSIC_SCRIPT.starts_with("on run argv") && !MUSIC_SCRIPT.contains("do shell script"));
        assert!(MUSIC_SCRIPT.is_ascii());
    }

    #[test]
    fn the_script_compiles() {
        if !cfg!(target_os = "macos") {
            return;
        }
        // Compiling reads Music's dictionary without running anything in Music.
        let out = std::env::temp_dir().join(format!("alfred-music-{}.scpt", std::process::id()));
        let mut cmd = Command::new("osacompile");
        cmd.arg("-e").arg(MUSIC_SCRIPT).arg("-o").arg(&out);
        let result = run(cmd, Duration::from_secs(30));
        std::fs::remove_file(&out).ok();
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn answers_are_read_into_their_parts() {
        assert_eq!(parse("notrunning\n"), Answer::NotRunning);
        assert_eq!(parse("paused\n"), Answer::Paused);
        assert_eq!(
            parse(&format!("playing{SEP}Get Lucky{SEP}Daft Punk{SEP}\n")),
            Answer::Track { playing: true, name: "Get Lucky".into(), artist: "Daft Punk".into(), playlist: String::new() }
        );
        assert_eq!(
            parse(&format!("paused{SEP}Get Lucky{SEP}{SEP}\n")),
            Answer::Track { playing: false, name: "Get Lucky".into(), artist: String::new(), playlist: String::new() },
            "a paused track is not the pause command's answer"
        );
        assert_eq!(parse(&format!("playlists{SEP}Focus{RS}Running{RS}\n")), Answer::Playlists(vec!["Focus".into(), "Running".into()]));
        assert_eq!(parse(&format!("playlists{SEP}\n")), Answer::Playlists(vec![]));
    }

    #[test]
    fn what_alfred_says_names_the_song_and_the_playlist() {
        let track = |playlist: &str| Answer::Track {
            playing: true,
            name: "Get Lucky".into(),
            artist: "Daft Punk".into(),
            playlist: playlist.into(),
        };
        assert_eq!(summary(MusicCommand::Play, "get lucky", &track("")), "Playing “Get Lucky” by Daft Punk.");
        assert_eq!(
            summary(MusicCommand::Play, "running", &track("Running")),
            "Playing your playlist “Running”, starting with “Get Lucky” by Daft Punk."
        );
        assert_eq!(summary(MusicCommand::Next, "", &track("")), "Now playing “Get Lucky” by Daft Punk.");
        assert_eq!(summary(MusicCommand::NowPlaying, "", &track("")), "This is “Get Lucky” by Daft Punk.");
        let paused = Answer::Track { playing: false, name: "Get Lucky".into(), artist: String::new(), playlist: String::new() };
        assert_eq!(summary(MusicCommand::NowPlaying, "", &paused), "“Get Lucky” is paused.");
        assert_eq!(summary(MusicCommand::Pause, "", &Answer::NotRunning), "Nothing is playing.");
        assert_eq!(
            summary(MusicCommand::Play, "Sabrina Carpenter", &Answer::NotFound),
            "I didn't find “Sabrina Carpenter” in your library, so I've opened Apple Music's search for it."
        );
        let many: Vec<String> = (1..=14).map(|i| format!("P{i}")).collect();
        assert!(summary(MusicCommand::Playlists, "", &Answer::Playlists(many)).ends_with("P12, and 2 more."));
        assert_eq!(
            summary(MusicCommand::Playlists, "", &Answer::Playlists(vec!["Focus".into(), "Running".into(), "Chill".into()])),
            "You have 3 playlists: Focus, Running and Chill."
        );
    }

    #[test]
    fn a_song_said_with_its_artist_is_split() {
        assert_eq!(split_by("Discovery by Daft Punk"), ("Discovery", "Daft Punk"));
        assert_eq!(split_by("Stand By Me"), ("Stand", "Me"), "the script tries the whole name first");
        assert_eq!(split_by("Focus"), ("Focus", ""));
        assert_eq!(split_by("by"), ("by", ""));
        assert_eq!(catalogue_search("Daft Punk & co"), "music://music.apple.com/search?term=Daft%20Punk%20%26%20co");
    }
}
