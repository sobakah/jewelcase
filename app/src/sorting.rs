//! The sort orders of the album and artist grids (the `album-sort` and
//! `artist-sort` settings). The library and its Spotify section (saved
//! albums, followed artists) are sorted by the same rules, each on its own
//! (CONCEPT.md §9.8).

use std::cmp::Ordering;

use jewelcase_core::db::{Album, Artist, SpotifyAlbum, SpotifyArtist};
use jewelcase_core::library::titles::sort_key;

/// What an album is sorted by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumKey {
    pub artist: String,
    pub title: String,
    pub year: Option<i32>,
    /// Larger is newer.
    pub added: i64,
}

impl AlbumKey {
    pub fn local(album: &Album) -> Self {
        Self {
            artist: album.artist_sort.clone(),
            title: album.sort_title.clone(),
            year: album.year,
            added: album.added,
        }
    }

    /// `position` is the album's place in Spotify's list of saved albums,
    /// which has the most recently saved first.
    pub fn spotify(album: &SpotifyAlbum, position: usize) -> Self {
        Self {
            artist: sort_key(&album.artist),
            title: sort_key(&album.name),
            year: album.year,
            added: -(position as i64),
        }
    }
}

/// `sort` is the `album-sort` setting: "artist" (then year, then title),
/// "title", "year" (newest first) or "added" (newest first).
pub fn compare_albums(sort: &str, a: &AlbumKey, b: &AlbumKey) -> Ordering {
    let by_artist = || {
        a.artist
            .cmp(&b.artist)
            .then(a.year.cmp(&b.year))
            .then(a.title.cmp(&b.title))
    };
    match sort {
        "title" => a.title.cmp(&b.title).then_with(by_artist),
        "year" => b.year.cmp(&a.year).then_with(by_artist),
        "added" => b.added.cmp(&a.added).then_with(by_artist),
        _ => by_artist(),
    }
}

/// What an artist is sorted by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistKey {
    pub name: String,
    /// Larger is newer.
    pub added: i64,
}

impl ArtistKey {
    pub fn local(artist: &Artist) -> Self {
        Self {
            name: artist.sort_name.clone(),
            added: artist.added,
        }
    }

    /// `position` is the artist's place in Spotify's list of followed
    /// artists, which has the most recently followed first.
    pub fn spotify(artist: &SpotifyArtist, position: usize) -> Self {
        Self {
            name: sort_key(&artist.name),
            added: -(position as i64),
        }
    }
}

/// `sort` is the `artist-sort` setting: "name" or "added" (newest first).
pub fn compare_artists(sort: &str, a: &ArtistKey, b: &ArtistKey) -> Ordering {
    let by_name = || a.name.cmp(&b.name);
    match sort {
        "added" => b.added.cmp(&a.added).then_with(by_name),
        _ => by_name(),
    }
}

/// Sorts a Spotify list, kept in Spotify's order (newest first), by a key
/// made from each item and its place there.
pub fn sort_spotify<T, K>(
    items: Vec<T>,
    key: impl Fn(&T, usize) -> K,
    compare: impl Fn(&K, &K) -> Ordering,
) -> Vec<T> {
    let mut keyed: Vec<(K, T)> = items
        .into_iter()
        .enumerate()
        .map(|(position, item)| (key(&item, position), item))
        .collect();
    keyed.sort_by(|(a, _), (b, _)| compare(a, b));
    keyed.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(name: &str, artist: &str, year: i32) -> SpotifyAlbum {
        SpotifyAlbum {
            name: name.into(),
            artist: artist.into(),
            year: Some(year),
            ..SpotifyAlbum::default()
        }
    }

    fn names(albums: &[SpotifyAlbum]) -> Vec<&str> {
        albums.iter().map(|a| a.name.as_str()).collect()
    }

    /// Spotify's list, most recently saved first.
    fn spotify_albums() -> Vec<SpotifyAlbum> {
        vec![
            saved("Wallsocket", "underscores", 2023),
            saved("The Fame", "Lady Gaga", 2008),
            saved("Brat", "Charli xcx", 2024),
            saved("Product", "SOPHIE", 2015),
            saved("A Hard Day's Night", "The Beatles", 1964),
        ]
    }

    fn sorted_albums(sort: &str) -> Vec<SpotifyAlbum> {
        sort_spotify(spotify_albums(), AlbumKey::spotify, |a, b| {
            compare_albums(sort, a, b)
        })
    }

    #[test]
    fn saved_albums_follow_the_album_sort() {
        // By artist, without "The": Beatles, Charli, Lady Gaga, SOPHIE, underscores.
        assert_eq!(
            names(&sorted_albums("artist")),
            [
                "A Hard Day's Night",
                "Brat",
                "The Fame",
                "Product",
                "Wallsocket"
            ]
        );
        // By title, without the article: "Fame" between "Brat" and "Hard".
        assert_eq!(
            names(&sorted_albums("title")),
            [
                "Brat",
                "The Fame",
                "A Hard Day's Night",
                "Product",
                "Wallsocket"
            ]
        );
        assert_eq!(
            names(&sorted_albums("year")),
            [
                "Brat",
                "Wallsocket",
                "Product",
                "The Fame",
                "A Hard Day's Night"
            ]
        );
        // Date added keeps Spotify's order: the most recently saved first.
        assert_eq!(names(&sorted_albums("added")), names(&spotify_albums()));
    }

    #[test]
    fn local_albums_sort_as_before() {
        let album = |title: &str, artist: &str, year, added| AlbumKey {
            artist: artist.into(),
            title: title.into(),
            year: Some(year),
            added,
        };
        let mut albums = [
            album("25", "adele", 2015, 300),
            album("19", "adele", 2008, 100),
            album("1000 forms of fear", "sia", 2014, 200),
        ];
        albums.sort_by(|a, b| compare_albums("artist", a, b));
        assert_eq!(albums.map(|a| a.title), ["19", "25", "1000 forms of fear"]);
        let mut albums = [
            album("19", "adele", 2008, 100),
            album("25", "adele", 2015, 300),
            album("1000 forms of fear", "sia", 2014, 200),
        ];
        albums.sort_by(|a, b| compare_albums("added", a, b));
        assert_eq!(albums.map(|a| a.title), ["25", "1000 forms of fear", "19"]);
    }

    #[test]
    fn artists_sort_by_name_or_date_added() {
        // Spotify's list, most recently followed first.
        let followed = |name: &str| SpotifyArtist {
            name: name.into(),
            ..SpotifyArtist::default()
        };
        let spotify = vec![
            followed("SOPHIE"),
            followed("The Beatles"),
            followed("Arca"),
        ];
        let by = |sort: &str| {
            sort_spotify(spotify.clone(), ArtistKey::spotify, |a, b| {
                compare_artists(sort, a, b)
            })
            .into_iter()
            .map(|a| a.name)
            .collect::<Vec<_>>()
        };
        assert_eq!(by("name"), ["Arca", "The Beatles", "SOPHIE"]);
        assert_eq!(by("added"), ["SOPHIE", "The Beatles", "Arca"]);

        let local = |name: &str, added| ArtistKey {
            name: name.into(),
            added,
        };
        let mut artists = [
            local("adele", 100),
            local("sia", 200),
            local("beyonce", 200),
        ];
        artists.sort_by(|a, b| compare_artists("added", a, b));
        // Newest first; the same date by name.
        assert_eq!(artists.map(|a| a.name), ["beyonce", "sia", "adele"]);
    }
}
