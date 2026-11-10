//! Where lyrics are read from: sidecar files and embedded tags (§5).

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::id3::v2::{
    Frame, Id3v2Tag, SyncTextContentType, SynchronizedTextFrame, TimestampFormat,
};
use lofty::tag::ItemKey;

/// Sidecar suffixes of romanizations: `Song.ja-Latn.lrc`, `Song.romaji.lrc`.
const ROMANIZED_SUFFIXES: &[&str] = &["romaji", "roman", "romanized"];

/// Lyrics text from one tag field or frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLyrics {
    pub text: String,
    /// The field or frame marks it as a romanization.
    pub romanized: bool,
}

#[derive(Debug, Default)]
pub struct Embedded {
    pub texts: Vec<RawLyrics>,
    /// The language tag (`TLAN`, `LANGUAGE`), lowercase.
    pub language: Option<String>,
}

/// `Song.<ext>` next to the audio file.
pub fn sidecar(path: &Path, extension: &str) -> Option<String> {
    read_text(&path.with_extension(extension))
}

/// `Song.<script>.lrc` with a Latin script subtag, or `Song.romaji.lrc`.
pub fn romanized_sidecar(path: &Path) -> Option<String> {
    let dir = path.parent()?;
    let stem = path.file_stem()?.to_str()?;
    let prefix = format!("{stem}.");
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(middle) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".lrc"))
        else {
            continue;
        };
        // Only a language or romanization tag between the song's name and
        // ".lrc": "01. Intro.ja-Latn.lrc" belongs to "01. Intro", not "01".
        if middle.contains('.') {
            continue;
        }
        let lower = middle.to_lowercase();
        if lower.ends_with("-latn") || ROMANIZED_SUFFIXES.contains(&lower.as_str()) {
            return read_text(&entry.path());
        }
    }
    None
}

fn read_text(path: &PathBuf) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// All lyrics fields of a file, in tag order.
pub fn embedded(path: &Path) -> Embedded {
    match read_embedded(path) {
        Ok(embedded) => embedded,
        Err(err) => {
            tracing::debug!("No embedded lyrics in {}: {err}", path.display());
            Embedded::default()
        }
    }
}

fn read_embedded(path: &Path) -> Result<Embedded, lofty::error::FileParseError> {
    let options = ParseOptions::new().read_cover_art(false);
    let tagged = lofty::probe::Probe::open(path)?.options(options).read()?;
    let file_type = tagged.file_type();
    let mut embedded = Embedded {
        language: tagged
            .primary_tag()
            .or_else(|| tagged.first_tag())
            .and_then(|t| t.get_string(ItemKey::Language))
            .map(|l| l.trim().to_lowercase()),
        ..Embedded::default()
    };

    // Format-specific fields keep descriptors and extra fields that the
    // generic tag merges or drops.
    let mut file = File::open(path)?;
    match file_type {
        FileType::Mpeg => {
            if let Ok(mpeg) = lofty::mpeg::MpegFile::read_from(&mut file, options)
                && let Some(id3) = mpeg.id3v2()
            {
                embedded.texts = id3_lyrics(id3);
            }
        }
        FileType::Flac => {
            if let Ok(flac) = lofty::flac::FlacFile::read_from(&mut file, options)
                && let Some(comments) = flac.vorbis_comments()
            {
                embedded.texts =
                    vorbis_lyrics(|key| comments.get_all(key).map(str::to_owned).collect());
            }
        }
        FileType::Vorbis => {
            if let Ok(ogg) = lofty::ogg::VorbisFile::read_from(&mut file, options) {
                let comments = ogg.vorbis_comments();
                embedded.texts =
                    vorbis_lyrics(|key| comments.get_all(key).map(str::to_owned).collect());
            }
        }
        // The same comments, after an Opus header (VorbisFile refuses it).
        FileType::Opus => {
            if let Ok(ogg) = lofty::ogg::OpusFile::read_from(&mut file, options) {
                let comments = ogg.vorbis_comments();
                embedded.texts =
                    vorbis_lyrics(|key| comments.get_all(key).map(str::to_owned).collect());
            }
        }
        _ => {}
    }
    if embedded.texts.is_empty() {
        // MP4 ©lyr and everything else: the generic lyrics field.
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
        if let Some(text) = tag.and_then(|t| t.get_string(ItemKey::Lyrics)) {
            embedded.texts.push(RawLyrics {
                text: text.to_owned(),
                romanized: false,
            });
        }
    }
    Ok(embedded)
}

/// Vorbis comments: `LYRICS`, `SYNCEDLYRICS`, `UNSYNCEDLYRICS`, plus
/// romanizations in `LYRICS-ROMANIZED` or `LYRICS:<lang>-Latn`.
fn vorbis_lyrics(get_all: impl Fn(&str) -> Vec<String>) -> Vec<RawLyrics> {
    let mut texts = Vec::new();
    for key in ["SYNCEDLYRICS", "LYRICS", "UNSYNCEDLYRICS"] {
        texts.extend(get_all(key).into_iter().map(|text| RawLyrics {
            text,
            romanized: false,
        }));
    }
    for key in [
        "LYRICS-ROMANIZED",
        "LYRICS:JA-LATN",
        "LYRICS:KO-LATN",
        "LYRICS:ZH-LATN",
    ] {
        texts.extend(get_all(key).into_iter().map(|text| RawLyrics {
            text,
            romanized: true,
        }));
    }
    texts
}

/// ID3v2: `SYLT` frames (converted to LRC) and `USLT` frames. A frame
/// whose description mentions a Latin script or romanization is one.
fn id3_lyrics(id3: &Id3v2Tag) -> Vec<RawLyrics> {
    let mut texts = Vec::new();
    for frame in id3 {
        match frame {
            Frame::UnsynchronizedText(uslt) => texts.push(RawLyrics {
                text: uslt.content.to_string(),
                romanized: is_romanized_label(&uslt.description),
            }),
            Frame::Binary(binary) if frame.id_str() == "SYLT" => {
                if let Ok(sylt) = SynchronizedTextFrame::parse(&binary.data, Default::default())
                    && let Some(lrc) = sylt_to_lrc(&sylt)
                {
                    texts.push(RawLyrics {
                        text: lrc,
                        romanized: sylt.description.as_deref().is_some_and(is_romanized_label),
                    });
                }
            }
            _ => {}
        }
    }
    // Synced first: SYLT is the most precise source.
    texts.sort_by_key(|raw| !crate::lrc::has_timestamps(&raw.text));
    texts
}

fn is_romanized_label(label: &str) -> bool {
    let lower = label.to_lowercase();
    lower.contains("latn") || lower.contains("roman")
}

fn sylt_to_lrc(sylt: &SynchronizedTextFrame) -> Option<String> {
    if sylt.timestamp_format != TimestampFormat::MS
        || !matches!(
            sylt.content_type,
            SyncTextContentType::Lyrics | SyncTextContentType::Other
        )
    {
        return None;
    }
    let mut lrc = String::new();
    for (ms, text) in &sylt.content {
        let (minutes, rest) = (ms / 60_000, ms % 60_000);
        lrc.push_str(&format!(
            "[{minutes:02}:{:02}.{:02}]{}\n",
            rest / 1000,
            rest % 1000 / 10,
            text.trim_matches(['\n', '\r'])
        ));
    }
    (!lrc.is_empty()).then_some(lrc)
}
