//! Display titles, editions, release types and genre keys (CONCEPT.md §3.4).
//!
//! Tags are never changed: these functions only decide what is shown and how
//! albums are grouped.

use std::time::Duration;

/// Release type of an album, used for filters and artist-page sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReleaseType {
    Album,
    Ep,
    Single,
}

impl ReleaseType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Ep => "ep",
            Self::Single => "single",
        }
    }

    pub fn from_str_lossy(value: &str) -> Self {
        match value {
            "ep" => Self::Ep,
            "single" => Self::Single,
            _ => Self::Album,
        }
    }
}

/// What a `RELEASETYPE` tag (MusicBrainz style, e.g. `album; soundtrack`) says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReleaseTypeTag {
    pub primary: Option<ReleaseType>,
    pub soundtrack: bool,
}

pub fn parse_release_type_tag(tag: &str) -> ReleaseTypeTag {
    let mut parsed = ReleaseTypeTag::default();
    for part in tag.split([';', '/', ',']).map(|p| p.trim().to_lowercase()) {
        match part.as_str() {
            "album" | "broadcast" | "other" => {
                parsed.primary.get_or_insert(ReleaseType::Album);
            }
            "ep" => {
                parsed.primary.get_or_insert(ReleaseType::Ep);
            }
            "single" => {
                parsed.primary.get_or_insert(ReleaseType::Single);
            }
            "soundtrack" => parsed.soundtrack = true,
            _ => {}
        }
    }
    parsed
}

/// Splits an iTunes ` - Single` / ` - EP` suffix off an album title.
pub fn split_itunes_suffix(title: &str) -> (&str, Option<ReleaseType>) {
    for (suffix, kind) in [
        (" - Single", ReleaseType::Single),
        (" - EP", ReleaseType::Ep),
    ] {
        if let Some(stripped) = strip_suffix_ignore_case(title, suffix) {
            let stripped = stripped.trim_end();
            if !stripped.is_empty() {
                return (stripped, Some(kind));
            }
        }
    }
    (title, None)
}

/// Moves recognized edition suffixes out of an album title:
/// "Blue Neighbourhood (Deluxe)" → ("Blue Neighbourhood", Some("Deluxe")).
///
/// Several suffixes are joined: "X (Deluxe) (Remastered)" → "Deluxe, Remastered".
pub fn split_edition(title: &str) -> (&str, Option<String>) {
    let mut rest = title.trim_end();
    let mut editions = Vec::new();
    while let Some((before, inner)) = split_trailing_brackets(rest) {
        if before.is_empty() || !is_edition(inner) {
            break;
        }
        editions.push(inner.trim());
        rest = before;
    }
    if editions.is_empty() {
        return (title, None);
    }
    editions.reverse();
    (rest, Some(editions.join(", ")))
}

/// For "Title (inner)" or "Title [inner]" returns ("Title", "inner").
fn split_trailing_brackets(text: &str) -> Option<(&str, &str)> {
    let (open, close) = match text.chars().last()? {
        ')' => ('(', ')'),
        ']' => ('[', ']'),
        _ => return None,
    };
    let mut depth = 0;
    for (index, ch) in text.char_indices().rev() {
        if ch == close {
            depth += 1;
        } else if ch == open {
            depth -= 1;
            if depth == 0 {
                let inner = &text[index + 1..text.len() - 1];
                return Some((text[..index].trim_end(), inner));
            }
        }
    }
    None
}

fn is_edition(inner: &str) -> bool {
    let inner = inner.trim().to_lowercase();
    const EXACT: &[&str] = &["deluxe", "remastered", "remaster"];
    const SUFFIXES: &[&str] = &[
        " ver.",
        " ver",
        " version",
        " edition",
        " remaster",
        " remastered",
    ];
    EXACT.contains(&inner.as_str())
        || SUFFIXES
            .iter()
            .any(|suffix| inner.len() > suffix.len() && inner.ends_with(suffix))
}

fn strip_suffix_ignore_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let split = text.len().checked_sub(suffix.len())?;
    let (head, tail) = (text.get(..split)?, text.get(split..)?);
    tail.eq_ignore_ascii_case(suffix).then_some(head)
}

/// The streaming-service rule for albums without a release type tag:
/// 1–3 tracks and < 30 min → Single, ≤ 6 tracks and < 30 min → EP, else Album.
pub fn release_type_from_length(track_count: usize, length: Duration) -> ReleaseType {
    let short = length < Duration::from_secs(30 * 60);
    match track_count {
        1..=3 if short => ReleaseType::Single,
        1..=6 if short => ReleaseType::Ep,
        _ => ReleaseType::Album,
    }
}

/// A key for grouping genres; never shown. "Hip-Hop/Rap" = "hip hop/rap",
/// "Soundtracks" = "soundtrack", "J-Pop " = "j pop".
pub fn genre_key(genre: &str) -> String {
    let unified: String = genre
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == '-' || c == '_' { ' ' } else { c })
        .collect();
    let mut words: Vec<&str> = unified.split_whitespace().collect();
    let singular;
    if let Some(last) = words.last_mut()
        && let Some(stripped) = last.strip_suffix('s')
        && stripped.len() >= 4
        && !stripped.ends_with('s')
        && !matches!(*last, "blues" | "lyrics" | "classics")
    {
        singular = stripped.to_owned();
        *last = &singular;
    }
    words.join(" ")
}

/// Whether a genre marks a soundtrack (§3.4 rule 4).
pub fn is_soundtrack_genre(genre: &str) -> bool {
    genre_key(genre) == "soundtrack"
}

/// Lowercase sort key without a leading English article.
pub fn sort_key(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article)
            && !rest.is_empty()
        {
            return rest.to_owned();
        }
    }
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edition_suffixes_from_the_library() {
        assert_eq!(
            split_edition("Blue Neighbourhood (Deluxe)"),
            ("Blue Neighbourhood", Some("Deluxe".into()))
        );
        assert_eq!(
            split_edition("Floral Shoppe (Remastered)"),
            ("Floral Shoppe", Some("Remastered".into()))
        );
        assert_eq!(
            split_edition("MAP OF THE SOUL : 7 (Day ver.)"),
            ("MAP OF THE SOUL : 7", Some("Day ver.".into()))
        );
        assert_eq!(
            split_edition("X (Deluxe Version) [Special Edition]"),
            ("X", Some("Deluxe Version, Special Edition".into()))
        );
    }

    #[test]
    fn other_brackets_stay_in_the_title() {
        assert_eq!(
            split_edition("Devilman Crybaby (Original Soundtrack)"),
            ("Devilman Crybaby (Original Soundtrack)", None)
        );
        assert_eq!(split_edition("(Deluxe)"), ("(Deluxe)", None));
        assert_eq!(
            split_edition("Who Cares (ver.)"),
            ("Who Cares (ver.)", None)
        );
        assert_eq!(split_edition("Broken (a"), ("Broken (a", None));
    }

    #[test]
    fn itunes_suffix() {
        assert_eq!(
            split_itunes_suffix("Ich hab Polizei - Single"),
            ("Ich hab Polizei", Some(ReleaseType::Single))
        );
        assert_eq!(
            split_itunes_suffix("Love - EP"),
            ("Love", Some(ReleaseType::Ep))
        );
        assert_eq!(split_itunes_suffix(" - EP"), (" - EP", None));
        assert_eq!(split_itunes_suffix("EP"), ("EP", None));
    }

    #[test]
    fn release_type_rule() {
        let min = |m: u64| Duration::from_secs(m * 60);
        assert_eq!(release_type_from_length(1, min(4)), ReleaseType::Single);
        assert_eq!(release_type_from_length(3, min(12)), ReleaseType::Single);
        assert_eq!(release_type_from_length(5, min(20)), ReleaseType::Ep);
        assert_eq!(release_type_from_length(6, min(31)), ReleaseType::Album);
        assert_eq!(release_type_from_length(7, min(25)), ReleaseType::Album);
        assert_eq!(release_type_from_length(2, min(40)), ReleaseType::Album);
    }

    #[test]
    fn release_type_tag() {
        assert_eq!(
            parse_release_type_tag("album; soundtrack"),
            ReleaseTypeTag {
                primary: Some(ReleaseType::Album),
                soundtrack: true
            }
        );
        assert_eq!(parse_release_type_tag("EP").primary, Some(ReleaseType::Ep));
        assert_eq!(parse_release_type_tag("live").primary, None);
    }

    #[test]
    fn genre_keys() {
        assert_eq!(genre_key("Hip-Hop/Rap"), genre_key("Hip Hop/Rap"));
        assert_eq!(genre_key("Soundtracks"), "soundtrack");
        assert_eq!(genre_key("J-Pop "), "j pop");
        assert_eq!(genre_key("K-POP"), "k pop");
        assert_eq!(genre_key("Blues"), "blues");
        assert!(is_soundtrack_genre("Soundtrack"));
    }

    #[test]
    fn sort_keys() {
        assert_eq!(sort_key("The Weeknd"), "weeknd");
        assert_eq!(sort_key("A"), "a");
        assert_eq!(sort_key("Adele"), "adele");
    }
}
