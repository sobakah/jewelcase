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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    /// Lines have timestamps.
    pub synced: bool,
    /// At least one line has a romanization.
    pub dual: bool,
    pub source: Source,
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
    }
}

fn unsynced(text: &str, source: Source) -> Lyrics {
    Lyrics {
        lines: lrc::plain(text),
        synced: false,
        dual: false,
        source,
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
}
