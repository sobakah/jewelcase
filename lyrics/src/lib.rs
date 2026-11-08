//! Lyrics for the player (CONCEPT.md §5): sidecar and embedded sources,
//! LRC parsing and dual-line pairing. Offline only; getting missing
//! lyrics is lyrembed's job.

pub mod lrc;
mod sources;

use std::path::Path;

pub use lrc::Line;

/// Where lyrics came from, for the pane's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Sidecar,
    Embedded,
    /// LRCLIB, for songs that aren't in the library (shown only).
    Online,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    /// Lines have timestamps.
    pub synced: bool,
    /// At least one line has a romanization.
    pub dual: bool,
    pub source: Source,
    /// The file's language tag (`TLAN`, `LANGUAGE`), lowercase.
    pub language: Option<String>,
}

impl Lyrics {
    /// Non-Latin lines without a romanization: worth romanizing for
    /// display (§5.1).
    pub fn needs_romanization(&self) -> bool {
        !self.dual
            && self
                .lines
                .iter()
                .any(|line| line.secondary.is_none() && !lrc::is_latin(&line.primary))
    }

    /// The lyrics as text again (LRC when synced), to romanize them.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for line in &self.lines {
            if let (true, Some(ms)) = (self.synced, line.time_ms) {
                text.push_str(&format!(
                    "[{:02}:{:02}.{:02}]",
                    ms / 60_000,
                    ms / 1000 % 60,
                    ms % 1000 / 10
                ));
            }
            text.push_str(&line.primary);
            text.push('\n');
        }
        text
    }

    /// What the language tag says about Han characters: "ja" or "zh".
    pub fn han_hint(&self) -> Option<&'static str> {
        match self.language.as_deref()? {
            "jpn" | "ja" => Some("ja"),
            "zho" | "chi" | "zh" | "cmn" | "yue" => Some("zh"),
            _ => None,
        }
    }

    /// These lyrics with the romanization lyrembed made (each romanized
    /// line under its original, §5.1).
    pub fn with_romanization(&self, romanized: &str) -> Lyrics {
        let lines = if self.synced {
            lrc::pair(lrc::parse(romanized))
        } else {
            pair_plain(&self.lines, romanized)
        };
        let dual = lines.iter().any(|l| l.secondary.is_some());
        Lyrics {
            lines,
            dual,
            ..self.clone()
        }
    }
}

/// Plain lyrics and their romanization: lyrembed puts a romanized line
/// right under each line it changed, so a Latin line that isn't the next
/// original line belongs to the one before.
fn pair_plain(original: &[Line], romanized: &str) -> Vec<Line> {
    let mut result = romanized.lines().map(str::trim).peekable();
    let mut lines = Vec::with_capacity(original.len());
    for (index, line) in original.iter().enumerate() {
        // The original line itself (skipped if lyrembed changed it).
        result.next_if(|r| *r == line.primary);
        let next_original = original.get(index + 1).map(|l| l.primary.as_str());
        let secondary = if lrc::is_latin(&line.primary) {
            None
        } else {
            result
                .next_if(|r| !r.is_empty() && Some(*r) != next_original && lrc::is_latin(r))
                .map(str::to_owned)
        };
        lines.push(Line {
            time_ms: None,
            primary: line.primary.clone(),
            secondary,
        });
    }
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LyricsState {
    Lyrics(Lyrics),
    Instrumental,
    None,
}

/// Loads the lyrics of an audio file, in the priority order of §5:
/// sidecar `.lrc`, embedded synced, embedded or sidecar plain text.
pub fn load(path: &Path, title: Option<&str>) -> LyricsState {
    let embedded = sources::embedded(path);
    match load_from(path, title, &embedded) {
        LyricsState::Lyrics(lyrics) => LyricsState::Lyrics(Lyrics {
            language: embedded.language.clone(),
            ..lyrics
        }),
        state => state,
    }
}

fn load_from(path: &Path, title: Option<&str>, embedded: &sources::Embedded) -> LyricsState {
    if embedded.language.as_deref() == Some("zxx") || title.is_some_and(is_instrumental_title) {
        return LyricsState::Instrumental;
    }

    let romanized_sidecar = sources::romanized_sidecar(path);
    if let Some(text) = sources::sidecar(path, "lrc").filter(|t| lrc::has_timestamps(t)) {
        return LyricsState::Lyrics(synced(&text, romanized_sidecar.as_deref(), Source::Sidecar));
    }

    let synced_texts: Vec<&sources::RawLyrics> = embedded
        .texts
        .iter()
        .filter(|raw| lrc::has_timestamps(&raw.text))
        .collect();
    if let Some(main) = synced_texts
        .iter()
        .find(|raw| !raw.romanized)
        .or(synced_texts.first())
    {
        let romanized = synced_texts
            .iter()
            .find(|raw| raw.romanized && raw.text != main.text)
            .map(|raw| raw.text.as_str())
            .or(romanized_sidecar.as_deref());
        return LyricsState::Lyrics(synced(&main.text, romanized, Source::Embedded));
    }

    if let Some(raw) = embedded
        .texts
        .iter()
        .find(|raw| !raw.text.trim().is_empty())
    {
        return LyricsState::Lyrics(unsynced(&raw.text, Source::Embedded));
    }
    if let Some(text) = sources::sidecar(path, "txt").filter(|t| !t.trim().is_empty()) {
        return LyricsState::Lyrics(unsynced(&text, Source::Sidecar));
    }
    LyricsState::None
}

/// Lyrics from an online source: synced text (LRC) if it has timestamps,
/// else plain text.
pub fn from_online(
    synced_text: Option<&str>,
    plain: Option<&str>,
    instrumental: bool,
) -> LyricsState {
    if instrumental {
        return LyricsState::Instrumental;
    }
    if let Some(text) = synced_text.filter(|t| lrc::has_timestamps(t)) {
        return LyricsState::Lyrics(synced(text, None, Source::Online));
    }
    match plain.filter(|t| !t.trim().is_empty()) {
        Some(text) => LyricsState::Lyrics(unsynced(text, Source::Online)),
        None => LyricsState::None,
    }
}

fn synced(text: &str, romanized: Option<&str>, source: Source) -> Lyrics {
    let mut lines = lrc::pair(lrc::parse(text));
    if let Some(romanized) = romanized {
        lines = lrc::merge_romanization(lines, &lrc::parse(romanized));
    }
    let dual = lines.iter().any(|l| l.secondary.is_some());
    Lyrics {
        lines,
        synced: true,
        dual,
        source,
        language: None,
    }
}

fn unsynced(text: &str, source: Source) -> Lyrics {
    Lyrics {
        lines: lrc::plain(text),
        synced: false,
        dual: false,
        source,
        language: None,
    }
}

/// Version notes that mark instrumentals (following lyrembed's rules).
pub fn is_instrumental_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    const MARKERS: &[&str] = &[
        "(inst.)",
        "(inst)",
        "(instrumental)",
        "- instrumental",
        "(off vocal)",
        "(off-vocal)",
        "(mr)",
        "(karaoke)",
        "オフボーカル",
        "[inst.]",
        "[instrumental]",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn instrumental_titles() {
        assert!(is_instrumental_title("Dynamite (Instrumental)"));
        assert!(is_instrumental_title("Euphoria (Inst.)"));
        assert!(is_instrumental_title("夜に駆ける (オフボーカル)"));
        assert!(is_instrumental_title("Song (MR)"));
        assert!(!is_instrumental_title("Mr. Brightside"));
    }

    #[test]
    fn sidecars_win_and_pair_with_romanization_sidecars() {
        let dir = tempfile::TempDir::new().unwrap();
        let song = dir.path().join("Song.flac");
        fs::write(&song, b"not audio").unwrap();
        fs::write(dir.path().join("Song.lrc"), "[00:01.00]東京\n[00:02.00]夜").unwrap();
        fs::write(
            dir.path().join("Song.ja-Latn.lrc"),
            "[00:01.00]Tōkyō\n[00:02.00]Yoru",
        )
        .unwrap();

        let LyricsState::Lyrics(lyrics) = load(&song, Some("Song")) else {
            panic!("expected lyrics");
        };
        assert_eq!(lyrics.source, Source::Sidecar);
        assert!(lyrics.synced && lyrics.dual);
        assert_eq!(lyrics.lines[1].secondary.as_deref(), Some("Yoru"));
    }

    #[test]
    fn txt_sidecar_is_unsynced_and_missing_is_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let song = dir.path().join("Song.mp3");
        fs::write(&song, b"").unwrap();
        assert_eq!(load(&song, None), LyricsState::None);
        fs::write(dir.path().join("Song.txt"), "Line one\nLine two").unwrap();
        let LyricsState::Lyrics(lyrics) = load(&song, None) else {
            panic!("expected lyrics");
        };
        assert!(!lyrics.synced);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(load(&song, Some("Song (Inst.)")), LyricsState::Instrumental);
    }

    #[test]
    fn online_lyrics_prefer_synced_text() {
        let LyricsState::Lyrics(lyrics) = from_online(
            Some("[00:01.00]Hello\n[00:03.00]World"),
            Some("Hello\nWorld"),
            false,
        ) else {
            panic!("no lyrics");
        };
        assert!(lyrics.synced);
        assert_eq!(lyrics.source, Source::Online);
        assert_eq!(lyrics.lines.len(), 2);
        let LyricsState::Lyrics(plain) = from_online(None, Some("Hello"), false) else {
            panic!("no lyrics");
        };
        assert!(!plain.synced);
        assert_eq!(from_online(None, None, true), LyricsState::Instrumental);
        assert_eq!(from_online(Some(""), Some("  "), false), LyricsState::None);
    }

    #[test]
    fn romanization_from_lyrembed_pairs_with_the_lines() {
        let LyricsState::Lyrics(synced) =
            from_online(Some("[00:01.00]사랑해요\n[00:03.50]Hello"), None, false)
        else {
            panic!("no lyrics");
        };
        assert!(synced.needs_romanization());
        assert_eq!(synced.to_text(), "[00:01.00]사랑해요\n[00:03.50]Hello\n");
        // What `lyrembed romanize --stdio` answers.
        let paired =
            synced.with_romanization("[00:01.00]사랑해요\n[00:01.00]saranghaeyo\n[00:03.50]Hello");
        assert!(paired.dual && !paired.needs_romanization());
        assert_eq!(paired.lines.len(), 2);
        assert_eq!(paired.lines[0].secondary.as_deref(), Some("saranghaeyo"));
        assert_eq!(paired.lines[1].secondary, None);

        let LyricsState::Lyrics(plain) =
            from_online(None, Some("사랑해요\n\nHello\n밤하늘"), false)
        else {
            panic!("no lyrics");
        };
        let paired = plain.with_romanization("사랑해요\nsaranghaeyo\n\nHello\n밤하늘\nbamhaneul");
        let secondaries: Vec<Option<&str>> = paired
            .lines
            .iter()
            .map(|l| l.secondary.as_deref())
            .collect();
        assert_eq!(
            secondaries,
            [Some("saranghaeyo"), None, None, Some("bamhaneul")]
        );
    }

    #[test]
    fn the_language_tag_hints_han_characters() {
        let LyricsState::Lyrics(mut lyrics) = from_online(None, Some("約束"), false) else {
            panic!("no lyrics");
        };
        assert_eq!(lyrics.han_hint(), None);
        lyrics.language = Some("jpn".into());
        assert_eq!(lyrics.han_hint(), Some("ja"));
        lyrics.language = Some("zho".into());
        assert_eq!(lyrics.han_hint(), Some("zh"));
    }
}
