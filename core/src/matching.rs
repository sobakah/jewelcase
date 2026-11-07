//! Matching streaming tracks to library songs (CONCEPT.md §9.5): ISRC
//! first, then title + artist + duration, where version notes ("Inst.",
//! "Live", "Remix", "Sped Up", "Japanese ver.") must agree.
//!
//! The same comparison runs the other way when a library song is added to
//! a Spotify playlist (§9.6).

use std::collections::{BTreeSet, HashMap};

use crate::library::artists::{Aliases, track_artist_names};

/// A track from Spotify (or another source), as far as matching cares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteTrack {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: u64,
    pub isrc: Option<String>,
}

/// A library song, as far as matching cares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalTrack {
    pub track_id: i64,
    pub title: String,
    /// Track artist ("RM (feat. eAeon)").
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: u64,
    pub isrc: Option<String>,
    /// "FLAC", "MPEG-4", …; decides between several copies.
    pub format: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Shown as a suggestion; the user confirms.
    Likely,
    /// Used without asking.
    Sure,
}

/// Allowed duration difference for a sure match.
const SURE_DURATION_MS: u64 = 3000;
/// Up to this, title and artist agreeing make a suggestion.
const LIKELY_DURATION_MS: u64 = 10_000;

/// How sure we are that both are the same recording; `None` if they
/// aren't.
pub fn compare(local: &LocalTrack, remote: &RemoteTrack, aliases: &Aliases) -> Option<Confidence> {
    if let (Some(a), Some(b)) = (&local.isrc, &remote.isrc)
        && a.eq_ignore_ascii_case(b)
    {
        return Some(Confidence::Sure);
    }
    let local_title = Title::parse(&local.title);
    let remote_title = Title::parse(&remote.title);
    if local_title.base.is_empty() || local_title.base != remote_title.base {
        return None;
    }
    // "(Inst.)" never matches the plain version, and so on.
    if local_title.notes != remote_title.notes {
        return None;
    }
    let difference = local.duration_ms.abs_diff(remote.duration_ms);
    let artist = artists_agree(local, remote, aliases);
    match (artist, difference) {
        (true, d) if d <= SURE_DURATION_MS => Some(Confidence::Sure),
        (true, d) if d <= LIKELY_DURATION_MS => Some(Confidence::Likely),
        // A romanized name the aliases don't know, say.
        (false, d) if d <= SURE_DURATION_MS => Some(Confidence::Likely),
        _ => None,
    }
}

fn artists_agree(local: &LocalTrack, remote: &RemoteTrack, aliases: &Aliases) -> bool {
    let remote: BTreeSet<String> = remote
        .artists
        .iter()
        .flat_map(|a| {
            let mut names = track_artist_names(a);
            names.push(a.clone());
            names
        })
        .map(|name| artist_key(&name, aliases))
        .collect();
    [&local.artist, &local.album_artist]
        .into_iter()
        .flatten()
        .flat_map(|a| {
            let mut names = track_artist_names(a);
            names.push(a.clone());
            names
        })
        .any(|name| remote.contains(&artist_key(&name, aliases)))
}

/// Alias identity, without spaces and punctuation ("TOMORROW X TOGETHER"
/// = "TXT" through aliases; "Day6" = "DAY6"; "Ne-Yo" = "NeYo").
fn artist_key(name: &str, aliases: &Aliases) -> String {
    let identity = aliases.identity(name);
    identity.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// A title split into what's compared: the base and the version notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Title {
    /// Lowercase, without brackets, credits and punctuation.
    pub base: String,
    pub notes: BTreeSet<String>,
}

impl Title {
    pub fn parse(title: &str) -> Self {
        let mut rest = title.to_lowercase();
        let mut extras = Vec::new();
        // Bracketed parts: "(feat. X)", "[Inst.]", "(Butterfly)".
        while let Some(open) = rest.find(['(', '[', '{', '（', '【']) {
            let close = rest[open..]
                .find([')', ']', '}', '）', '】'])
                .map(|c| open + c);
            let Some(close) = close else { break };
            let close_len = rest[close..].chars().next().map_or(1, char::len_utf8);
            extras.push(
                rest[open..close]
                    .trim_start_matches(['(', '[', '{', '（', '【'])
                    .to_owned(),
            );
            rest.replace_range(open..close + close_len, " ");
        }
        // Spotify style suffixes: "Song - Live at Wembley", "Song - 2011 Remaster".
        while let Some(dash) = rest.rfind(" - ") {
            extras.push(rest[dash + 3..].to_owned());
            rest.truncate(dash);
        }
        // "Song feat. X" outside brackets.
        for marker in [" feat. ", " feat ", " ft. ", " featuring "] {
            if let Some(at) = rest.find(marker) {
                rest.truncate(at);
            }
        }
        let notes = extras.iter().filter_map(|e| version_note(e)).collect();
        let base = rest
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>();
        Self { base, notes }
    }
}

/// What a bracket or suffix says about the version; `None` for things
/// that don't change the recording (credits, remasters, subtitles).
fn version_note(extra: &str) -> Option<String> {
    let text = extra.trim().trim_end_matches('.');
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect();
    let has = |word: &str| words.contains(&word);
    if words.is_empty()
        || [
            "feat",
            "ft",
            "featuring",
            "with",
            "prod",
            "from",
            "remastered",
            "remaster",
        ]
        .iter()
        .any(|w| words.first() == Some(w))
    {
        return None;
    }
    if has("inst")
        || has("instrumental")
        || has("karaoke")
        || has("mr")
        || has("off") && has("vocal")
    {
        return Some("inst".into());
    }
    if text.contains("sped up") || text.contains("speed up") || has("nightcore") {
        return Some("sped up".into());
    }
    if has("slowed") {
        return Some("slowed".into());
    }
    if has("acoustic") || has("unplugged") {
        return Some("acoustic".into());
    }
    if has("acapella") || text.contains("a cappella") || has("acappella") {
        return Some("acapella".into());
    }
    if has("live") {
        return Some("live".into());
    }
    if has("demo") {
        return Some("demo".into());
    }
    // Remixes differ from each other: keep who remixed.
    if has("remix") || has("mix") && !has("original") || has("rmx") {
        let who: String = text.chars().filter(|c| c.is_alphanumeric()).collect();
        return Some(format!("remix {who}"));
    }
    // "Japanese ver.", "English Version", "Taylor's Version"; plain
    // edition words are the same recording.
    if has("ver") || has("version") {
        let neutral = [
            "original", "album", "single", "radio", "explicit", "clean", "main",
        ];
        if words.iter().any(|w| neutral.contains(w)) {
            return None;
        }
        let what: Vec<&str> = words
            .iter()
            .copied()
            .filter(|w| *w != "ver" && *w != "version")
            .collect();
        return Some(format!("ver {}", what.join(" ")));
    }
    None
}

/// The library, indexed for matching many entries at once.
pub struct MatchIndex<'a> {
    by_isrc: HashMap<String, Vec<usize>>,
    by_title: HashMap<String, Vec<usize>>,
    tracks: Vec<LocalTrack>,
    aliases: &'a Aliases,
}

impl<'a> MatchIndex<'a> {
    pub fn new(tracks: Vec<LocalTrack>, aliases: &'a Aliases) -> Self {
        let mut by_isrc: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, track) in tracks.iter().enumerate() {
            if let Some(isrc) = &track.isrc {
                by_isrc.entry(isrc.to_uppercase()).or_default().push(i);
            }
            let base = Title::parse(&track.title).base;
            if !base.is_empty() {
                by_title.entry(base).or_default().push(i);
            }
        }
        Self {
            by_isrc,
            by_title,
            tracks,
            aliases,
        }
    }

    /// The best library copy of `remote` and how sure the match is.
    pub fn find(&self, remote: &RemoteTrack) -> Option<(i64, Confidence)> {
        let mut candidates: Vec<usize> = remote
            .isrc
            .as_ref()
            .and_then(|isrc| self.by_isrc.get(&isrc.to_uppercase()))
            .cloned()
            .unwrap_or_default();
        if let Some(same_title) = self.by_title.get(&Title::parse(&remote.title).base) {
            candidates.extend(same_title);
        }
        let remote_album = remote.album.as_deref().map(album_key);
        candidates
            .into_iter()
            .filter_map(|i| {
                let track = &self.tracks[i];
                let confidence = compare(track, remote, self.aliases)?;
                let same_album =
                    remote_album.is_some() && track.album.as_deref().map(album_key) == remote_album;
                Some((
                    confidence,
                    same_album,
                    quality(&track.format),
                    track.track_id,
                ))
            })
            // Surest, then the copy on the same album, then the best file
            // (§9.5: several local copies).
            .max_by_key(|(confidence, same_album, quality, id)| {
                (*confidence, *same_album, *quality, std::cmp::Reverse(*id))
            })
            .map(|(confidence, _, _, id)| (id, confidence))
    }
}

fn album_key(album: &str) -> String {
    Title::parse(album).base
}

fn quality(format: &str) -> u8 {
    match format {
        "FLAC" | "ALAC" | "WAV" | "AIFF" => 3,
        "MPEG-4" | "Opus" | "Vorbis" => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(id: i64, title: &str, artist: &str, duration_s: u64) -> LocalTrack {
        LocalTrack {
            track_id: id,
            title: title.into(),
            artist: Some(artist.into()),
            album_artist: Some(artist.into()),
            album: Some("Album".into()),
            duration_ms: duration_s * 1000,
            isrc: None,
            format: "FLAC".into(),
        }
    }

    fn remote(title: &str, artists: &[&str], duration_s: u64) -> RemoteTrack {
        RemoteTrack {
            title: title.into(),
            artists: artists.iter().map(|a| (*a).to_owned()).collect(),
            album: None,
            duration_ms: duration_s * 1000,
            isrc: None,
        }
    }

    #[test]
    fn titles_are_cleaned_but_versions_kept() {
        let plain = Title::parse("Dynamite");
        assert_eq!(Title::parse("Dynamite (feat. Someone)"), plain);
        assert_eq!(Title::parse("DYNAMITE - 2020 Remaster"), plain);
        assert_eq!(Title::parse("Dynamite (Original Version)"), plain);
        assert_eq!(Title::parse("Dynamite feat. X"), plain);
        assert_ne!(Title::parse("Dynamite (Inst.)"), plain);
        assert_eq!(
            Title::parse("Dynamite (Instrumental)").notes,
            Title::parse("Dynamite [Inst.]").notes
        );
        assert_ne!(Title::parse("Dynamite - Live"), plain);
        assert_ne!(Title::parse("Dynamite (Sped Up)"), plain);
        assert_ne!(Title::parse("Dynamite (Japanese ver.)"), plain);
        assert_ne!(
            Title::parse("Dynamite (Tropical Remix)"),
            Title::parse("Dynamite (EDM Remix)")
        );
        // Localized subtitles don't matter.
        assert_eq!(Title::parse("Butterfly (봄날)").base, "butterfly");
    }

    #[test]
    fn matches_need_title_artist_and_duration() {
        let aliases = Aliases::parse_lyrembed("  TOMORROW X TOGETHER = TXT\n");
        let track = local(1, "Sugar Rush Ride", "TOMORROW X TOGETHER", 185);
        let aliased = remote("Sugar Rush Ride", &["TXT"], 186);
        assert_eq!(compare(&track, &aliased, &aliases), Some(Confidence::Sure));
        assert_eq!(
            compare(
                &track,
                &remote("Sugar Rush Ride", &["Someone"], 186),
                &aliases
            ),
            Some(Confidence::Likely)
        );
        assert_eq!(
            compare(&track, &remote("Sugar Rush Ride", &["TXT"], 192), &aliases),
            Some(Confidence::Likely)
        );
        assert_eq!(
            compare(&track, &remote("Sugar Rush Ride", &["TXT"], 230), &aliases),
            None
        );
        assert_eq!(
            compare(
                &track,
                &remote("Sugar Rush Ride (Inst.)", &["TXT"], 185),
                &aliases
            ),
            None
        );
        // Featured artists in the local tag.
        let featuring = local(2, "Wildflower", "RM (feat. youjeen)", 273);
        assert_eq!(
            compare(
                &featuring,
                &remote("Wildflower (with youjeen)", &["RM", "youjeen"], 273),
                &aliases
            ),
            Some(Confidence::Sure)
        );
    }

    #[test]
    fn isrc_wins() {
        let aliases = Aliases::default();
        let mut track = local(1, "Something else", "Nobody", 10);
        track.isrc = Some("KRA382000123".into());
        let mut entry = remote("Different", &["Other"], 300);
        entry.isrc = Some("kra382000123".into());
        assert_eq!(compare(&track, &entry, &aliases), Some(Confidence::Sure));
    }

    #[test]
    fn the_index_prefers_the_same_album_then_the_better_file() {
        let aliases = Aliases::default();
        let mut single = local(1, "Hype Boy", "NewJeans", 179);
        single.album = Some("Hype Boy".into());
        let mut album = local(2, "Hype Boy", "NewJeans", 179);
        album.album = Some("New Jeans".into());
        album.format = "MPEG-4".into();
        let mut mp3 = local(3, "Hype Boy", "NewJeans", 179);
        mp3.album = Some("Other".into());
        mp3.format = "MPEG".into();
        let index = MatchIndex::new(vec![single, album, mp3], &aliases);
        let mut entry = remote("Hype Boy", &["NewJeans"], 180);
        entry.album = Some("New Jeans".into());
        assert_eq!(index.find(&entry), Some((2, Confidence::Sure)));
        entry.album = None;
        assert_eq!(index.find(&entry), Some((1, Confidence::Sure)));
        assert_eq!(index.find(&remote("Attention", &["NewJeans"], 180)), None);
    }
}
