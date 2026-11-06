//! The album-artist model (CONCEPT.md §3.3).
//!
//! - Only album artists get entries. The tag decides: `ALBUMARTISTS`
//!   values, else the `ALBUMARTIST` credit as one entry. Strings are never
//!   split into new artists.
//! - Identity ignores case and lyrembed alias spellings ("TXT" =
//!   "TOMORROW X TOGETHER"). Different MusicBrainz IDs keep homonyms apart.
//!   The display name is the most frequent spelling.
//! - "Various Artists" and compilations get no entry.
//! - Parts of joint credits ("Get Well Soon & Kat Frankie") and of track
//!   artists ("RM (feat. eAeon)") are only *linked* to artists that already
//!   exist: collaborations and "Appears on".

use std::collections::{BTreeMap, HashMap, HashSet};

use super::TrackRecord;
use super::grouping::AlbumInfo;
use super::titles;

/// Separators inside joint credits (§3.3 rule 3.3). Deliberately not a
/// plain "x", because of "TOMORROW X TOGETHER".
const CREDIT_SEPARATORS: &[&str] = &[" & ", ", ", " × ", " / ", " and ", " und "];

/// Markers that start featured artists inside a track artist string.
const FEATURING: &[&str] = &["feat.", "feat ", "ft.", "featuring "];

/// Markers of credits that aren't performers ("RM (prod. HONNE)").
const NON_PERFORMER: &[&str] = &["prod.", "prod ", "produced by "];

/// Artist alias groups, from `lyrembed --list-aliases` (CONCEPT.md §10.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Aliases {
    /// Lowercase spelling → lowercase canonical spelling.
    canonical: HashMap<String, String>,
    /// Lowercase canonical spelling → every spelling as written.
    groups: HashMap<String, Vec<String>>,
}

impl Aliases {
    /// Parses lines like `  BTS = Bangtan Boys = 방탄소년단`; other lines
    /// (headers, notes) are skipped.
    pub fn parse_lyrembed(output: &str) -> Self {
        let mut aliases = Self::default();
        for line in output.lines() {
            if !line.starts_with(' ') || !line.contains(" = ") {
                continue;
            }
            let names: Vec<String> = line
                .split(" = ")
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty())
                .collect();
            let Some(first) = names.first().map(|n| n.to_lowercase()) else {
                continue;
            };
            for name in &names {
                aliases
                    .canonical
                    .entry(name.to_lowercase())
                    .or_insert_with(|| first.clone());
            }
            aliases.groups.entry(first).or_insert(names);
        }
        aliases
    }

    /// Runs `<program> --list-aliases`; empty if that fails.
    pub fn from_program(program: &std::path::Path) -> Self {
        match std::process::Command::new(program)
            .arg("--list-aliases")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        {
            Ok(output) if output.status.success() => {
                Self::parse_lyrembed(&String::from_utf8_lossy(&output.stdout))
            }
            Ok(output) => {
                tracing::info!("lyrembed --list-aliases failed: {}", output.status);
                Self::default()
            }
            Err(err) => {
                tracing::debug!("lyrembed is not available: {err}");
                Self::default()
            }
        }
    }

    /// `lyrembed` from `PATH`.
    pub fn from_lyrembed() -> Self {
        Self::from_program(std::path::Path::new("lyrembed"))
    }

    pub fn len(&self) -> usize {
        self.canonical.len()
    }

    pub fn is_empty(&self) -> bool {
        self.canonical.is_empty()
    }

    /// The identity of a name: lowercase, with aliases resolved.
    pub fn identity(&self, name: &str) -> String {
        let lower = name.trim().to_lowercase();
        self.canonical.get(&lower).cloned().unwrap_or(lower)
    }

    /// Every known spelling of a name (at least the name itself), for
    /// search: "Hoshino Gen" finds 星野源.
    pub fn spellings(&self, name: &str) -> Vec<String> {
        match self.groups.get(&self.identity(name)) {
            Some(group) => group.clone(),
            None => vec![name.to_owned()],
        }
    }

    /// A Latin-script spelling for a native-script name, shown as the
    /// artist page's subtitle.
    pub fn latin_name(&self, name: &str) -> Option<String> {
        if name.chars().all(|c| c.is_ascii() || !c.is_alphabetic()) {
            return None;
        }
        self.groups
            .get(&self.identity(name))?
            .iter()
            .find(|spelling| spelling.chars().all(|c| c.is_ascii() || !c.is_alphabetic()))
            .cloned()
    }
}

/// How an album belongs to an artist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AlbumRole {
    /// The artist is (one of) the album artist(s).
    Credit,
    /// The artist is part of a joint credit ("Get Well Soon & Kat Frankie").
    Collaboration,
}

impl AlbumRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Credit => "credit",
            Self::Collaboration => "collaboration",
        }
    }

    pub fn from_str_lossy(value: &str) -> Self {
        match value {
            "collaboration" => Self::Collaboration,
            _ => Self::Credit,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtistInfo {
    /// Stable identity: `name:<identity>`, plus `#<mbid>` for homonyms.
    pub key: String,
    pub name: String,
    pub sort_name: String,
    pub mbid: Option<String>,
    /// Album keys with role and position in the credit.
    pub albums: Vec<(String, AlbumRole, usize)>,
    /// Tracks on other artists' albums ("Appears on").
    pub appears_on: Vec<i64>,
}

/// One album-artist entry of one album.
struct Occurrence<'a> {
    album: &'a AlbumInfo,
    position: usize,
    name: &'a str,
    mbid: Option<&'a str>,
}

pub fn build_artists(
    albums: &[AlbumInfo],
    tracks: &[TrackRecord],
    aliases: &Aliases,
) -> Vec<ArtistInfo> {
    // 1. Album-artist occurrences, grouped by identity (and MBID).
    let mut by_identity: BTreeMap<String, Vec<Occurrence>> = BTreeMap::new();
    for album in albums.iter().filter(|a| !a.compilation) {
        let mbids_match = album.album_artist_mbids.len() == album.album_artists.len();
        for (position, name) in album.album_artists.iter().enumerate() {
            if name.trim().is_empty() || is_various_artists(name) {
                continue;
            }
            let mbid = mbids_match.then(|| album.album_artist_mbids[position].as_str());
            if mbid == Some(super::grouping::VARIOUS_ARTISTS_MBID) {
                continue;
            }
            by_identity
                .entry(aliases.identity(name))
                .or_default()
                .push(Occurrence {
                    album,
                    position,
                    name,
                    mbid,
                });
        }
    }

    let mut artists: Vec<ArtistInfo> = Vec::new();
    for (identity, occurrences) in by_identity {
        artists.extend(split_homonyms(&identity, occurrences));
    }

    // 2. Links to existing artists only.
    let index: HashMap<String, usize> = {
        let mut index = HashMap::new();
        for (i, artist) in artists.iter().enumerate() {
            // With homonyms, a plain name links to the first of them.
            index.entry(aliases.identity(&artist.name)).or_insert(i);
        }
        index
    };
    let lookup = |name: &str| index.get(&aliases.identity(name)).copied();

    for album in albums.iter().filter(|a| !a.compilation) {
        let credited: HashSet<usize> = album
            .album_artists
            .iter()
            .filter_map(|name| lookup(name))
            .collect();
        if album.album_artists.len() == 1 {
            for part in split_credit(&album.album_artists[0]) {
                if let Some(i) = lookup(&part)
                    && !credited.contains(&i)
                    && !artists[i].albums.iter().any(|(k, _, _)| *k == album.key)
                {
                    artists[i]
                        .albums
                        .push((album.key.clone(), AlbumRole::Collaboration, 0));
                }
            }
        }
    }

    let album_of: HashMap<i64, &AlbumInfo> = albums
        .iter()
        .flat_map(|a| a.track_ids.iter().map(move |id| (*id, a)))
        .collect();
    for track in tracks {
        let (Some(artist), Some(album)) = (&track.tags.artist, album_of.get(&track.id)) else {
            continue;
        };
        let on_own_album: HashSet<usize> = album
            .album_artists
            .iter()
            .flat_map(|name| {
                // Members of a joint credit are "on their own album" too.
                std::iter::once(name.clone()).chain(split_credit(name))
            })
            .filter_map(|name| lookup(&name))
            .collect();
        for name in track_artist_names(artist) {
            if let Some(i) = lookup(&name)
                && !on_own_album.contains(&i)
                && !artists[i].appears_on.contains(&track.id)
            {
                artists[i].appears_on.push(track.id);
            }
        }
    }
    artists
}

/// Occurrences of one identity become one artist, unless they carry
/// different MusicBrainz IDs. Occurrences without an MBID go to the most
/// frequent one.
fn split_homonyms(identity: &str, occurrences: Vec<Occurrence>) -> Vec<ArtistInfo> {
    let mut mbid_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for o in &occurrences {
        if let Some(mbid) = o.mbid {
            *mbid_counts.entry(mbid).or_default() += 1;
        }
    }
    let main_mbid = mbid_counts
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(mbid, _)| *mbid);

    let mut groups: BTreeMap<Option<&str>, Vec<&Occurrence>> = BTreeMap::new();
    for o in &occurrences {
        groups.entry(o.mbid.or(main_mbid)).or_default().push(o);
    }
    let homonyms = groups.len() > 1;
    groups
        .into_iter()
        .map(|(mbid, occurrences)| {
            let name = most_common(occurrences.iter().map(|o| o.name)).to_owned();
            let key = match (homonyms, mbid) {
                (true, Some(mbid)) if Some(mbid) != main_mbid => format!("name:{identity}#{mbid}"),
                _ => format!("name:{identity}"),
            };
            let mut albums: Vec<(String, AlbumRole, usize)> = occurrences
                .iter()
                .map(|o| (o.album.key.clone(), AlbumRole::Credit, o.position))
                .collect();
            albums.dedup();
            ArtistInfo {
                key,
                sort_name: titles::sort_key(&name),
                name,
                mbid: mbid.map(str::to_owned),
                albums,
                appears_on: Vec::new(),
            }
        })
        .collect()
}

fn most_common<'a>(names: impl Iterator<Item = &'a str>) -> &'a str {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for name in names {
        match counts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, count)) => *count += 1,
            None => counts.push((name, 1)),
        }
    }
    let max = counts.iter().map(|(_, c)| *c).max().unwrap_or(0);
    counts
        .into_iter()
        .find(|(_, c)| *c == max)
        .map_or("", |(n, _)| n)
}

pub fn is_various_artists(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("various artists")
}

/// The parts of a joint credit; empty if it isn't one.
pub fn split_credit(credit: &str) -> Vec<String> {
    let mut parts = vec![credit.to_owned()];
    for separator in CREDIT_SEPARATORS {
        parts = parts
            .iter()
            .flat_map(|part| split_ignore_case(part, separator))
            .collect();
    }
    let parts: Vec<String> = parts
        .into_iter()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() > 1 { parts } else { Vec::new() }
}

/// Performer names in a track artist string, e.g.
/// "RM (feat. eAeon)" → [RM, eAeon], "RM  (prod. HONNE)" → [RM],
/// "Bang Chan, Changbin, HAN" → [Bang Chan, Changbin, HAN].
pub fn track_artist_names(artist: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut rest = artist.to_owned();
    // Bracketed credits first: "(feat. X)", "[prod. Y]".
    while let Some(open) = rest.find(['(', '[']) {
        let close = rest[open..].find([')', ']']).map(|c| open + c);
        let Some(close) = close else { break };
        let inner = rest[open + 1..close].to_owned();
        segments.push(inner);
        rest.replace_range(open..=close, " ");
    }
    segments.insert(0, rest);

    let mut names = Vec::new();
    for segment in segments {
        let lower = segment.to_lowercase();
        if NON_PERFORMER
            .iter()
            .any(|m| lower.trim_start().starts_with(m))
        {
            continue;
        }
        // "A feat. B" outside brackets.
        let mut pieces = vec![segment.clone()];
        for marker in FEATURING {
            pieces = pieces
                .iter()
                .flat_map(|piece| split_ignore_case(piece, marker))
                .collect();
        }
        for piece in pieces {
            let piece = piece.trim();
            let parts = split_credit(piece);
            if parts.is_empty() {
                if !piece.is_empty() {
                    names.push(piece.to_owned());
                }
            } else {
                names.extend(parts);
            }
        }
    }
    names
}

fn split_ignore_case(text: &str, separator: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths; only split when it doesn't.
    if lower.len() != text.len() {
        return text.split(separator).map(str::to_owned).collect();
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let sep = separator.to_lowercase();
    while let Some(found) = lower[start..].find(&sep) {
        parts.push(text[start..start + found].to_owned());
        start += found + sep.len();
    }
    parts.push(text[start..].to_owned());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::grouping::group_albums;
    use crate::library::tags::TrackTags;
    use std::path::PathBuf;

    fn track(id: i64, folder: &str, album: &str, album_artist: &str, artist: &str) -> TrackRecord {
        TrackRecord {
            id,
            path: PathBuf::from(format!("{folder}/{id:02}.flac")),
            tags: TrackTags {
                title: Some(format!("Song {id}")),
                album: Some(album.into()),
                album_artist: Some(album_artist.into()),
                artist: Some(artist.into()),
                number: Some(id as u32),
                duration_ms: 200_000,
                ..TrackTags::default()
            },
            cover_key: None,
            added: 0,
        }
    }

    fn aliases() -> Aliases {
        Aliases::parse_lyrembed(
            "Built-in: 2 groups\n\n  TOMORROW X TOGETHER = TXT = 투모로우바이투게더\n  BTS = Bangtan Boys\n",
        )
    }

    fn by_name<'a>(artists: &'a [ArtistInfo], name: &str) -> &'a ArtistInfo {
        artists.iter().find(|a| a.name == name).unwrap()
    }

    #[test]
    fn parses_lyrembed_aliases() {
        let aliases = aliases();
        assert_eq!(aliases.identity("TXT"), "tomorrow x together");
        assert_eq!(
            aliases.identity("투모로우바이투게더"),
            "tomorrow x together"
        );
        assert_eq!(aliases.identity("Adele"), "adele");
        assert_eq!(aliases.len(), 5);
        assert_eq!(
            aliases.spellings("txt"),
            ["TOMORROW X TOGETHER", "TXT", "투모로우바이투게더"]
        );
        assert_eq!(aliases.spellings("Adele"), ["Adele"]);
        assert_eq!(
            aliases.latin_name("투모로우바이투게더").as_deref(),
            Some("TOMORROW X TOGETHER")
        );
        assert_eq!(aliases.latin_name("BTS"), None);
    }

    #[test]
    fn identity_ignores_case_and_aliases() {
        let tracks = vec![
            track(
                1,
                "/m/TXT/A",
                "A",
                "TOMORROW X TOGETHER",
                "TOMORROW X TOGETHER",
            ),
            track(
                2,
                "/m/TXT/B",
                "B",
                "TOMORROW X TOGETHER",
                "TOMORROW X TOGETHER",
            ),
            track(3, "/m/TXT/C", "C", "TXT", "TXT"),
            track(4, "/m/Y/D", "D", "YEONJUN", "YEONJUN"),
        ];
        let albums = group_albums(&tracks);
        let artists = build_artists(&albums, &tracks, &aliases());
        assert_eq!(artists.len(), 2);
        let txt = by_name(&artists, "TOMORROW X TOGETHER");
        assert_eq!(txt.albums.len(), 3);
        assert_eq!(txt.key, "name:tomorrow x together");
    }

    #[test]
    fn joint_credit_is_one_artist_with_collaboration_links() {
        let tracks = vec![
            track(
                1,
                "/m/GWS/S",
                "Schulz & Böhmermann",
                "Get Well Soon & Kat Frankie",
                "Get Well Soon",
            ),
            track(2, "/m/KF/B", "Bad Behaviour", "Kat Frankie", "Kat Frankie"),
            track(
                3,
                "/m/SG/X",
                "Bookends",
                "Simon & Garfunkel",
                "Simon & Garfunkel",
            ),
        ];
        let albums = group_albums(&tracks);
        let artists = build_artists(&albums, &tracks, &Aliases::default());
        let names: HashSet<&str> = artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            HashSet::from([
                "Get Well Soon & Kat Frankie",
                "Kat Frankie",
                "Simon & Garfunkel"
            ])
        );
        let kat = by_name(&artists, "Kat Frankie");
        assert_eq!(kat.albums.len(), 2);
        assert!(
            kat.albums
                .iter()
                .any(|(_, role, _)| *role == AlbumRole::Collaboration)
        );
        // Kat's track on her own collaboration isn't "Appears on".
        assert!(kat.appears_on.is_empty());
    }

    #[test]
    fn appears_on_links_track_artists_to_existing_artists_only() {
        let tracks = vec![
            track(
                1,
                "/m/TXT/A",
                "A",
                "TOMORROW X TOGETHER",
                "TOMORROW X TOGETHER",
            ),
            track(2, "/m/TXT/A", "A", "TOMORROW X TOGETHER", "YEONJUN"),
            track(3, "/m/Y/N", "NO LABELS", "YEONJUN", "YEONJUN"),
            track(4, "/m/RM/M", "mono.", "RM", "RM (feat. eAeon)"),
            track(5, "/m/RM/M", "mono.", "RM", "RM  (prod. HONNE)"),
        ];
        let albums = group_albums(&tracks);
        let artists = build_artists(&albums, &tracks, &Aliases::default());
        assert_eq!(artists.len(), 3, "eAeon and HONNE get no entry");
        assert_eq!(by_name(&artists, "YEONJUN").appears_on, vec![2]);
        assert!(by_name(&artists, "RM").appears_on.is_empty());
    }

    #[test]
    fn various_artists_and_compilations_get_no_entry() {
        let mut tracks = vec![
            track(1, "/m/VA/A", "Hits", "Various Artists", "Adele"),
            track(2, "/m/A/25", "25", "Adele", "Adele"),
        ];
        tracks[0].tags.compilation = true;
        let albums = group_albums(&tracks);
        let artists = build_artists(&albums, &tracks, &Aliases::default());
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].appears_on, vec![1]);
    }

    #[test]
    fn homonyms_with_different_mbids_stay_apart() {
        let mut tracks = vec![
            track(1, "/m/N/A", "A", "Nirvana", "Nirvana"),
            track(2, "/m/N/B", "B", "Nirvana", "Nirvana"),
            track(3, "/m/N/C", "C", "Nirvana", "Nirvana"),
        ];
        tracks[0].tags.mb_album_artist_ids = vec!["us-band".into()];
        tracks[1].tags.mb_album_artist_ids = vec!["uk-band".into()];
        tracks[2].tags.mb_album_artist_ids = vec!["us-band".into()];
        let albums = group_albums(&tracks);
        let artists = build_artists(&albums, &tracks, &Aliases::default());
        assert_eq!(artists.len(), 2);
        let keys: HashSet<&str> = artists.iter().map(|a| a.key.as_str()).collect();
        assert!(keys.contains("name:nirvana"));
        assert!(keys.contains("name:nirvana#uk-band"));
    }

    #[test]
    fn track_artist_strings_from_the_library() {
        assert_eq!(track_artist_names("RM (feat. eAeon)"), ["RM", "eAeon"]);
        assert_eq!(track_artist_names("RM  (prod. HONNE)"), ["RM"]);
        assert_eq!(
            track_artist_names("Alligatoah, Battleboi Basti"),
            ["Alligatoah", "Battleboi Basti"]
        );
        assert_eq!(
            track_artist_names("Bang Chan, Changbin, HAN"),
            ["Bang Chan", "Changbin", "HAN"]
        );
        assert_eq!(track_artist_names("A feat. B & C"), ["A", "B", "C"]);
        assert_eq!(
            track_artist_names("TOMORROW X TOGETHER"),
            ["TOMORROW X TOGETHER"]
        );
    }

    #[test]
    fn credit_splitting() {
        assert_eq!(
            split_credit("Get Well Soon & Kat Frankie"),
            ["Get Well Soon", "Kat Frankie"]
        );
        assert_eq!(split_credit("TOMORROW X TOGETHER"), Vec::<String>::new());
        assert_eq!(split_credit("Simon und Garfunkel"), ["Simon", "Garfunkel"]);
    }
}
