//! Playlist files: M3U/M3U8 and XSPF, import and export (CONCEPT.md §13,
//! phase 5).
//!
//! Imported entries keep what the file says (location, title, artist,
//! album, duration); matching them to library tracks is the database's
//! job, the same way playlist entries are re-linked after files move.

use std::path::{Component, Path, PathBuf};

use quick_xml::events::Event;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub location: Option<PathBuf>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlaylistFileError {
    #[error("cannot read the playlist: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid XSPF playlist: {0}")]
    Xml(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    M3u8,
    Xspf,
}

impl Format {
    pub fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .as_deref()
        {
            Some("xspf") => Self::Xspf,
            _ => Self::M3u8,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::M3u8 => "m3u8",
            Self::Xspf => "xspf",
        }
    }
}

/// Reads a playlist file: its name (the XSPF title or the file name) and
/// entries.
pub fn read(path: &Path) -> Result<(String, Vec<Entry>), PlaylistFileError> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let base = path.parent().unwrap_or(Path::new("/"));
    let file_name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let looks_like_xml = text.trim_start().starts_with('<');
    if Format::from_path(path) == Format::Xspf || looks_like_xml {
        let (title, entries) = parse_xspf(&text, base)?;
        Ok((title.unwrap_or(file_name), entries))
    } else {
        Ok((file_name, parse_m3u(&text, base)))
    }
}

/// M3U and M3U8: one location per line, `#EXTINF:<seconds>,<artist> - <title>`
/// before it. Relative paths are relative to the playlist's folder.
pub fn parse_m3u(text: &str, base: &Path) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut pending = Entry::default();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            let (duration, label) = info.split_once(',').unwrap_or((info, ""));
            pending.duration_ms = duration
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|d| *d > 0.0)
                .map(|d| (d * 1000.0) as u64);
            match label.split_once(" - ") {
                Some((artist, title)) => {
                    pending.artist = non_empty(artist);
                    pending.title = non_empty(title);
                }
                None => pending.title = non_empty(label),
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        pending.location = resolve_location(line, base);
        entries.push(std::mem::take(&mut pending));
    }
    entries
}

pub fn parse_xspf(
    text: &str,
    base: &Path,
) -> Result<(Option<String>, Vec<Entry>), PlaylistFileError> {
    let mut reader = quick_xml::Reader::from_str(text);
    let mut entries = Vec::new();
    let mut title = None;
    let mut current: Option<Entry> = None;
    let mut element = String::new();
    // Text and entity references (`&amp;`) arrive as separate events.
    let mut value = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                element = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                value.clear();
                if element == "track" {
                    current = Some(Entry::default());
                }
            }
            Ok(Event::Text(text)) => {
                if let Ok(text) = text.decode() {
                    value.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                let name = String::from_utf8_lossy(reference.as_ref()).into_owned();
                value.push_str(&resolve_entity(&name));
            }
            Ok(Event::End(end)) => {
                let name = end.local_name();
                let text = value.trim();
                match (&mut current, name.as_ref()) {
                    (Some(_), b"track") => {
                        if let Some(entry) = current.take() {
                            entries.push(entry);
                        }
                    }
                    (Some(entry), b"location") if entry.location.is_none() => {
                        entry.location = resolve_location(text, base);
                    }
                    (Some(entry), b"title") => entry.title = non_empty(text),
                    (Some(entry), b"creator") => entry.artist = non_empty(text),
                    (Some(entry), b"album") => entry.album = non_empty(text),
                    (Some(entry), b"duration") => entry.duration_ms = text.parse().ok(),
                    (None, b"title") => title = non_empty(text),
                    _ => {}
                }
                value.clear();
                element.clear();
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(PlaylistFileError::Xml(err.to_string())),
            _ => {}
        }
    }
    Ok((title, entries))
}

/// Writes M3U8 with `#EXTINF` lines and absolute paths.
pub fn write_m3u8(entries: &[Entry]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for entry in entries {
        let Some(location) = &entry.location else {
            continue;
        };
        let seconds = entry.duration_ms.map_or(-1, |ms| (ms / 1000) as i64);
        let label = match (&entry.artist, &entry.title) {
            (Some(artist), Some(title)) => format!("{artist} - {title}"),
            (None, Some(title)) => title.clone(),
            _ => String::new(),
        };
        out.push_str(&format!(
            "#EXTINF:{seconds},{label}\n{}\n",
            location.display()
        ));
    }
    out
}

/// Writes XSPF (version 1) with file URIs. Entries without a file keep
/// their title and artist, so other players can still match them.
pub fn write_xspf(name: &str, entries: &[Entry]) -> String {
    let esc = |s: &str| quick_xml::escape::escape(s).into_owned();
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<playlist version=\"1\" xmlns=\"http://xspf.org/ns/0/\">\n",
    );
    out.push_str(&format!("  <title>{}</title>\n  <trackList>\n", esc(name)));
    for entry in entries {
        out.push_str("    <track>\n");
        if let Some(location) = &entry.location {
            out.push_str(&format!(
                "      <location>{}</location>\n",
                esc(&file_uri(location))
            ));
        }
        for (tag, value) in [
            ("title", &entry.title),
            ("creator", &entry.artist),
            ("album", &entry.album),
        ] {
            if let Some(value) = value {
                out.push_str(&format!("      <{tag}>{}</{tag}>\n", esc(value)));
            }
        }
        if let Some(ms) = entry.duration_ms {
            out.push_str(&format!("      <duration>{ms}</duration>\n"));
        }
        out.push_str("    </track>\n");
    }
    out.push_str("  </trackList>\n</playlist>\n");
    out
}

/// `amp` → `&`, `#228` / `#xE4` → `ä`.
fn resolve_entity(name: &str) -> String {
    match name {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        _ => name
            .strip_prefix("#x")
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
            .and_then(char::from_u32)
            .map_or_else(String::new, String::from),
    }
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A playlist line or `<location>` as a local path: `file://` URIs,
/// absolute and relative paths. Remote URLs are ignored.
fn resolve_location(location: &str, base: &Path) -> Option<PathBuf> {
    let location = location.trim();
    let path = if let Some(rest) = location.strip_prefix("file://") {
        // file:///path or file://localhost/path
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        PathBuf::from(percent_decode(rest)?)
    } else if location.contains("://") {
        return None;
    } else {
        let decoded = location.replace('\\', "/");
        let path = PathBuf::from(decoded);
        if path.is_absolute() {
            path
        } else {
            base.join(path)
        }
    };
    Some(normalize(&path))
}

/// Removes `.` and `..` without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `file://` URI with percent-encoding (RFC 3986 unreserved and `/` kept).
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(*byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m3u_with_extinf_relative_paths_and_uris() {
        let text = "#EXTM3U\n#EXTINF:230,BTS - Euphoria\n../BTS/Answer/01 Euphoria.flac\n\n#EXTINF:-1,Just a title\n/m/Adele/25/01 Hello.m4a\nfile:///m/A%20B/c%C3%A4.mp3\nhttp://radio.example/stream\n";
        let entries = parse_m3u(text, Path::new("/m/Playlists"));
        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries[0].location,
            Some(PathBuf::from("/m/BTS/Answer/01 Euphoria.flac"))
        );
        assert_eq!(entries[0].artist.as_deref(), Some("BTS"));
        assert_eq!(entries[0].title.as_deref(), Some("Euphoria"));
        assert_eq!(entries[0].duration_ms, Some(230_000));
        assert_eq!(entries[1].title.as_deref(), Some("Just a title"));
        assert_eq!(entries[1].duration_ms, None);
        assert_eq!(entries[2].location, Some(PathBuf::from("/m/A B/cä.mp3")));
        assert_eq!(entries[3].location, None);
    }

    #[test]
    fn xspf_round_trip() {
        let entries = vec![
            Entry {
                location: Some(PathBuf::from("/m/Adele/25/01 Hello & Bye.m4a")),
                title: Some("Hello & Bye".into()),
                artist: Some("Adele".into()),
                album: Some("25".into()),
                duration_ms: Some(295_000),
            },
            Entry {
                location: None,
                title: Some("Elsewhere <live>".into()),
                artist: None,
                album: None,
                duration_ms: None,
            },
        ];
        let xml = write_xspf("Mix & Match", &entries);
        assert!(xml.contains("file:///m/Adele/25/01%20Hello%20%26%20Bye.m4a"));
        let (title, parsed) = parse_xspf(&xml, Path::new("/")).unwrap();
        assert_eq!(title.as_deref(), Some("Mix & Match"));
        assert_eq!(parsed, entries);
    }

    #[test]
    fn m3u8_export() {
        let entries = vec![Entry {
            location: Some(PathBuf::from("/m/a.flac")),
            title: Some("A".into()),
            artist: Some("B".into()),
            album: None,
            duration_ms: Some(61_500),
        }];
        assert_eq!(
            write_m3u8(&entries),
            "#EXTM3U\n#EXTINF:61,B - A\n/m/a.flac\n"
        );
        let parsed = parse_m3u(&write_m3u8(&entries), Path::new("/"));
        assert_eq!(parsed[0].location, entries[0].location);
    }

    #[test]
    fn reads_files_by_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("Road Trip.m3u");
        std::fs::write(&path, "song.flac\n").unwrap();
        let (name, entries) = read(&path).unwrap();
        assert_eq!(name, "Road Trip");
        assert_eq!(entries[0].location, Some(dir.path().join("song.flac")));
    }
}
