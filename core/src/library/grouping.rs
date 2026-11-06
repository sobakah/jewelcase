//! Groups tracks into albums and merges editions (CONCEPT.md §3.3, §3.4).

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use super::TrackRecord;
use super::titles::{self, ReleaseType};

/// MusicBrainz artist ID of "Various Artists".
pub const VARIOUS_ARTISTS_MBID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

/// Durations of the same song in two editions may differ by this much.
const SAME_TRACK_TOLERANCE_MS: u64 = 2000;

/// One album as computed from its tracks.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumInfo {
    /// Album identity: the MusicBrainz release ID, else album + album artist + folder.
    pub key: String,
    /// Albums with the same group key are editions of one album; the key is
    /// the identity of the group's first (preferred) version.
    pub group_key: String,
    /// Display title without edition and iTunes suffixes.
    pub title: String,
    pub sort_title: String,
    pub edition: Option<String>,
    /// The album-artist credit as tagged. Empty if no track says.
    pub artist_credit: String,
    pub artist_sort: String,
    /// One entry per album artist (§3.3): `ALBUMARTISTS` values, else the credit.
    pub album_artists: Vec<String>,
    pub year: Option<i32>,
    pub release_type: ReleaseType,
    pub soundtrack: bool,
    pub compilation: bool,
    pub mb_album_id: Option<String>,
    pub mb_release_group_id: Option<String>,
    /// In play order: disc, track number, title.
    pub track_ids: Vec<i64>,
    pub duration_ms: u64,
    pub cover_key: Option<String>,
    pub added: i64,
}

pub fn group_albums(tracks: &[TrackRecord]) -> Vec<AlbumInfo> {
    let mut by_key: HashMap<String, Vec<&TrackRecord>> = HashMap::new();
    for track in tracks {
        by_key.entry(identity_key(track)).or_default().push(track);
    }

    let mut albums: Vec<(AlbumInfo, Vec<&TrackRecord>)> = by_key
        .into_iter()
        .map(|(key, mut tracks)| {
            tracks.sort_by(|a, b| play_order(a).cmp(&play_order(b)));
            (build_album(key, &tracks), tracks)
        })
        .collect();
    // Preferred version first: no edition, then editions alphabetically.
    albums.sort_by(|(a, _), (b, _)| {
        (a.edition.is_some(), &a.edition, &a.key).cmp(&(b.edition.is_some(), &b.edition, &b.key))
    });

    merge_editions(&mut albums);
    albums.into_iter().map(|(album, _)| album).collect()
}

fn identity_key(track: &TrackRecord) -> String {
    let tags = &track.tags;
    if let Some(mbid) = &tags.mb_album_id {
        return format!("mb:{}", mbid.to_lowercase());
    }
    let folder = album_folder(&track.path);
    let title = tags
        .album
        .clone()
        .unwrap_or_else(|| folder_name(folder).to_owned());
    let artist = tags.album_artist.as_deref().unwrap_or("");
    format!(
        "tag:{}\u{1f}{}\u{1f}{}",
        title.to_lowercase(),
        artist.to_lowercase(),
        folder.display()
    )
}

/// The folder an album lives in. Disc subfolders ("CD1", "Disc 2") belong
/// to their parent, so multi-disc albums stay together.
pub fn album_folder(path: &Path) -> &Path {
    let parent = path.parent().unwrap_or(path);
    if is_disc_folder(folder_name(parent)) {
        parent.parent().unwrap_or(parent)
    } else {
        parent
    }
}

fn folder_name(folder: &Path) -> &str {
    folder.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

fn is_disc_folder(name: &str) -> bool {
    let lower = name.to_lowercase();
    let rest = ["cd", "disc", "disk"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix));
    rest.map(|r| r.trim_start_matches([' ', '_', '-']))
        .is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))
}

fn play_order(track: &TrackRecord) -> (u32, u32, String, &Path) {
    (
        track.tags.disc.unwrap_or(1),
        track.tags.number.unwrap_or(u32::MAX),
        track.tags.title.clone().unwrap_or_default().to_lowercase(),
        &track.path,
    )
}

fn build_album(key: String, tracks: &[&TrackRecord]) -> AlbumInfo {
    let most_common = |f: &dyn Fn(&TrackRecord) -> Option<String>| {
        most_common(tracks.iter().filter_map(|t| f(t)))
    };

    let raw_title = most_common(&|t| t.tags.album.clone())
        .unwrap_or_else(|| folder_name(album_folder(&tracks[0].path)).to_owned());
    let (without_suffix, itunes_type) = titles::split_itunes_suffix(&raw_title);
    let (title, title_edition) = titles::split_edition(without_suffix);
    let title = title.to_owned();
    let edition = most_common(&|t| t.tags.release_comment.clone()).or(title_edition);

    let artist_credit = most_common(&|t| t.tags.album_artist.clone())
        .or_else(|| most_common(&|t| t.tags.artist.clone()))
        .unwrap_or_default();
    let album_artists = most_common_list(tracks.iter().map(|t| &t.tags.album_artists))
        .unwrap_or_else(|| {
            if artist_credit.is_empty() {
                Vec::new()
            } else {
                vec![artist_credit.clone()]
            }
        });
    let artist_sort = most_common(&|t| t.tags.album_artist_sort.clone())
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| titles::sort_key(&artist_credit));

    let duration_ms = tracks.iter().map(|t| t.tags.duration_ms).sum();
    let type_tag = tracks
        .iter()
        .find_map(|t| t.tags.release_type.as_deref())
        .map(titles::parse_release_type_tag)
        .unwrap_or_default();
    let release_type = type_tag.primary.or(itunes_type).unwrap_or_else(|| {
        titles::release_type_from_length(tracks.len(), Duration::from_millis(duration_ms))
    });
    let soundtrack = type_tag.soundtrack
        || tracks
            .iter()
            .flat_map(|t| &t.tags.genres)
            .any(|g| titles::is_soundtrack_genre(g));
    let compilation = tracks.iter().any(|t| {
        t.tags.compilation
            || t.tags
                .mb_album_artist_ids
                .iter()
                .any(|id| id == VARIOUS_ARTISTS_MBID)
    }) || artist_credit.eq_ignore_ascii_case("various artists");

    AlbumInfo {
        sort_title: titles::sort_key(&title),
        title,
        edition,
        album_artists,
        artist_sort,
        artist_credit,
        year: most_common(&|t| t.tags.year.map(|y| y.to_string())).and_then(|y| y.parse().ok()),
        release_type,
        soundtrack,
        compilation,
        mb_album_id: most_common(&|t| t.tags.mb_album_id.clone()),
        mb_release_group_id: most_common(&|t| t.tags.mb_release_group_id.clone()),
        track_ids: tracks.iter().map(|t| t.id).collect(),
        duration_ms,
        cover_key: tracks.iter().find_map(|t| t.cover_key.clone()),
        added: tracks.iter().map(|t| t.added).min().unwrap_or_default(),
        group_key: key.clone(),
        key,
    }
}

/// Same title + album artist + same tracklist → one album group.
fn merge_editions(albums: &mut [(AlbumInfo, Vec<&TrackRecord>)]) {
    // Indices of each group's first album, per (title, artist).
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for index in 0..albums.len() {
        let bucket = (
            albums[index].0.title.to_lowercase(),
            albums[index].0.artist_credit.to_lowercase(),
        );
        let firsts = groups.entry(bucket).or_default();
        let matching = firsts
            .iter()
            .copied()
            .find(|&first| same_tracklist(&albums[first].1, &albums[index].1));
        match matching {
            Some(first) => albums[index].0.group_key = albums[first].0.key.clone(),
            None => firsts.push(index),
        }
    }
}

fn same_tracklist(a: &[&TrackRecord], b: &[&TrackRecord]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let title = |t: &TrackRecord| t.tags.title.as_deref().unwrap_or("").to_lowercase();
            title(x) == title(y)
                && x.tags.duration_ms.abs_diff(y.tags.duration_ms) <= SAME_TRACK_TOLERANCE_MS
        })
}

/// The most frequent value; ties go to the first seen.
fn most_common(values: impl Iterator<Item = String>) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for value in values {
        match counts.iter_mut().find(|(v, _)| *v == value) {
            Some((_, count)) => *count += 1,
            None => counts.push((value, 1)),
        }
    }
    let max = counts.iter().map(|(_, c)| *c).max()?;
    counts.into_iter().find(|(_, c)| *c == max).map(|(v, _)| v)
}

fn most_common_list<'a>(lists: impl Iterator<Item = &'a Vec<String>>) -> Option<Vec<String>> {
    let joined = lists.filter(|l| !l.is_empty()).map(|l| l.join("\u{1f}"));
    most_common(joined).map(|j| j.split('\u{1f}').map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::tags::TrackTags;
    use std::path::PathBuf;

    fn track(id: i64, path: &str, album: &str, title: &str, number: u32, secs: u64) -> TrackRecord {
        TrackRecord {
            id,
            path: PathBuf::from(path),
            tags: TrackTags {
                title: Some(title.into()),
                album: Some(album.into()),
                album_artist: Some("BTS".into()),
                number: Some(number),
                duration_ms: secs * 1000,
                ..TrackTags::default()
            },
            cover_key: None,
            added: id,
        }
    }

    fn by_key<'a>(albums: &'a [AlbumInfo], title: &str, edition: Option<&str>) -> &'a AlbumInfo {
        albums
            .iter()
            .find(|a| a.title == title && a.edition.as_deref() == edition)
            .unwrap()
    }

    #[test]
    fn editions_with_same_tracklist_are_merged() {
        let mut tracks = vec![
            track(
                1,
                "/m/BTS/Answer L/01.flac",
                "LOVE YOURSELF 結 'Answer'",
                "Euphoria",
                1,
                230,
            ),
            track(
                2,
                "/m/BTS/Answer L/02.flac",
                "LOVE YOURSELF 結 'Answer'",
                "Trivia",
                2,
                200,
            ),
            track(
                3,
                "/m/BTS/Answer F/01.flac",
                "LOVE YOURSELF 結 'Answer'",
                "Euphoria",
                1,
                231,
            ),
            track(
                4,
                "/m/BTS/Answer F/02.flac",
                "LOVE YOURSELF 結 'Answer'",
                "Trivia",
                2,
                199,
            ),
        ];
        tracks[0].tags.release_comment = Some("L version".into());
        tracks[1].tags.release_comment = Some("L version".into());
        tracks[2].tags.release_comment = Some("F version".into());
        tracks[3].tags.release_comment = Some("F version".into());

        let albums = group_albums(&tracks);
        assert_eq!(albums.len(), 2);
        let f = by_key(&albums, "LOVE YOURSELF 結 'Answer'", Some("F version"));
        let l = by_key(&albums, "LOVE YOURSELF 結 'Answer'", Some("L version"));
        // "F version" sorts first, so it's the group's preferred version.
        assert_eq!(f.group_key, f.key);
        assert_eq!(l.group_key, f.key);
        assert_eq!(l.track_ids, vec![1, 2]);
    }

    #[test]
    fn deluxe_with_bonus_tracks_stays_separate() {
        let tracks = vec![
            track(
                1,
                "/m/Troye/BN/01.m4a",
                "Blue Neighbourhood",
                "Wild",
                1,
                220,
            ),
            track(
                2,
                "/m/Troye/BND/01.m4a",
                "Blue Neighbourhood (Deluxe)",
                "Wild",
                1,
                220,
            ),
            track(
                3,
                "/m/Troye/BND/02.m4a",
                "Blue Neighbourhood (Deluxe)",
                "Bonus",
                2,
                180,
            ),
        ];
        let albums = group_albums(&tracks);
        assert_eq!(albums.len(), 2);
        let plain = by_key(&albums, "Blue Neighbourhood", None);
        let deluxe = by_key(&albums, "Blue Neighbourhood", Some("Deluxe"));
        assert_ne!(plain.group_key, deluxe.group_key);
    }

    #[test]
    fn disc_folders_belong_to_one_album() {
        let mut tracks = vec![
            track(1, "/m/OST/CD1/01.flac", "OST", "A", 1, 100),
            track(2, "/m/OST/Disc 2/01.flac", "OST", "B", 1, 100),
        ];
        tracks[1].tags.disc = Some(2);
        let albums = group_albums(&tracks);
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].track_ids, vec![1, 2]);
    }

    #[test]
    fn release_types_and_flags() {
        let mut single = track(
            1,
            "/m/A/S/01.m4a",
            "Ich hab Polizei - Single",
            "Ich hab Polizei",
            1,
            200,
        );
        single.tags.album_artist = Some("Various Artists".into());
        let mut ost = track(2, "/m/A/O/01.flac", "Cyberpunk 2077", "Main", 1, 2000);
        ost.tags.genres = vec!["Soundtracks".into()];
        let albums = group_albums(&[single, ost]);

        let single = by_key(&albums, "Ich hab Polizei", None);
        assert_eq!(single.release_type, ReleaseType::Single);
        assert!(single.compilation);
        let ost = by_key(&albums, "Cyberpunk 2077", None);
        assert_eq!(ost.release_type, ReleaseType::Album);
        assert!(ost.soundtrack);
    }

    #[test]
    fn album_artist_falls_back_to_track_artist_majority() {
        let mut tracks = vec![
            track(1, "/m/X/01.mp3", "X", "a", 1, 10),
            track(2, "/m/X/02.mp3", "X", "b", 2, 10),
            track(3, "/m/X/03.mp3", "X", "c", 3, 10),
        ];
        for (track, artist) in
            tracks
                .iter_mut()
                .zip(["Alligatoah", "Alligatoah, Battleboi Basti", "Alligatoah"])
        {
            track.tags.album_artist = None;
            track.tags.artist = Some(artist.into());
        }
        let albums = group_albums(&tracks);
        assert_eq!(albums[0].artist_credit, "Alligatoah");
        assert_eq!(albums[0].album_artists, vec!["Alligatoah".to_owned()]);
    }

    #[test]
    fn disc_folder_names() {
        assert!(is_disc_folder("CD1"));
        assert!(is_disc_folder("Disc 2"));
        assert!(is_disc_folder("disk_03"));
        assert!(!is_disc_folder("CD"));
        assert!(!is_disc_folder("Discovery"));
    }
}
