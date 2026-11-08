//! Listening statistics (CONCEPT.md §6.3): the play log and what played
//! most in a period, with the change against the period before.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};

use super::{Database, Result};
use crate::library::artists::track_artist_names;

/// One play from the log.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayEntry {
    /// Unix seconds.
    pub played: i64,
    pub ms_played: u64,
    /// A library song, or a streamed one as -stream_track.id.
    pub track_id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
}

/// How an entry moved against the period before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    /// Not among the previous period's plays.
    New,
    /// Places up (positive) or down (negative); 0: the same place.
    Moved(i32),
}

/// A song, artist, album or genre with its plays in the period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    pub name: String,
    /// The artist (songs and albums).
    pub artist: Option<String>,
    pub plays: u32,
    pub ms_played: u64,
    /// The song played last (for covers and playing it).
    pub track_id: i64,
    /// None when there's nothing to compare with (no earlier plays).
    pub trend: Option<Trend>,
}

/// What played in a period.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub plays: u32,
    pub ms_played: u64,
    pub songs: Vec<Ranked>,
    pub artists: Vec<Ranked>,
    pub albums: Vec<Ranked>,
    pub genres: Vec<Ranked>,
}

impl Database {
    /// Logs a play of `track_id` (a library song or `-stream_track.id`),
    /// copying what it is; songs that are neither aren't logged.
    pub fn log_play(&mut self, track_id: i64, played: i64, ms_played: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO play_log (played, ms_played, track_id, title, artist, album, album_artist, genre)
             SELECT ?2, ?3, id, coalesce(title, ''), artist, album, coalesce(album_artist, artist),
                    nullif(substr(genres, 1, instr(genres || char(31), char(31)) - 1), '')
             FROM track WHERE id = ?1
             UNION ALL
             SELECT ?2, ?3, -id, title, artist, album, artist, NULL
             FROM stream_track WHERE id = -?1",
            params![track_id, played, ms_played as i64],
        )?;
        Ok(())
    }

    /// The plays from `from` (inclusive) to `to` (exclusive), oldest first.
    pub fn play_log(&self, from: i64, to: i64) -> Result<Vec<PlayEntry>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT played, ms_played, track_id, title, artist, album, album_artist, genre
             FROM play_log WHERE played >= ?1 AND played < ?2 ORDER BY played, id",
        )?;
        let rows = stmt.query_map(params![from, to], |row| {
            Ok(PlayEntry {
                played: row.get(0)?,
                ms_played: row.get::<_, i64>(1)?.max(0) as u64,
                track_id: row.get(2)?,
                title: row.get(3)?,
                artist: row.get(4)?,
                album: row.get(5)?,
                album_artist: row.get(6)?,
                genre: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// When the first play was logged.
    pub fn first_play(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT min(played) FROM play_log", [], |row| row.get(0))
            .optional()?
            .flatten())
    }

    /// Deletes the whole log (Preferences → Privacy).
    pub fn delete_play_log(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM play_log", [])?;
        Ok(())
    }

    /// A library album by title and artist credit (any case), else by
    /// title alone when only one album has it: (id, cover key).
    pub fn album_named(
        &self,
        title: &str,
        artist: Option<&str>,
    ) -> Result<Option<(i64, Option<String>)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, cover_key, lower(artist_credit) = lower(?2) FROM album
             WHERE lower(title) = lower(?1) ORDER BY 3 DESC, id",
        )?;
        let found: Vec<(i64, Option<String>, bool)> = stmt
            .query_map(params![title, artist.unwrap_or_default()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(match found.as_slice() {
            [(id, cover, true), ..] | [(id, cover, false)] => Some((*id, cover.clone())),
            _ => None,
        })
    }

    /// A library artist by name (any case).
    pub fn artist_named(&self, name: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM artist WHERE lower(name) = lower(?1) ORDER BY id LIMIT 1",
                params![name],
                |row| row.get(0),
            )
            .optional()?)
    }
}

impl Summary {
    /// What played most in `current`, each list compared with `previous`
    /// (the period before; empty: no trends).
    pub fn new(current: &[PlayEntry], previous: &[PlayEntry]) -> Self {
        let compare = !previous.is_empty();
        let with_trend = |kind: Kind| {
            let mut now = tally(current, kind);
            if compare {
                let before: HashMap<String, usize> = tally(previous, kind)
                    .into_iter()
                    .enumerate()
                    .map(|(place, r)| (kind.key_of(&r), place))
                    .collect();
                for (place, entry) in now.iter_mut().enumerate() {
                    entry.trend = Some(match before.get(&kind.key_of(entry)) {
                        Some(&was) => Trend::Moved(was as i32 - place as i32),
                        None => Trend::New,
                    });
                }
            }
            now
        };
        Summary {
            plays: current.len() as u32,
            ms_played: current.iter().map(|e| e.ms_played).sum(),
            songs: with_trend(Kind::Song),
            artists: with_trend(Kind::Artist),
            albums: with_trend(Kind::Album),
            genres: with_trend(Kind::Genre),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Song,
    Artist,
    Album,
    Genre,
}

impl Kind {
    /// The groups a play counts for, as (key, name, artist).
    fn groups(self, entry: &PlayEntry) -> Vec<(String, String, Option<String>)> {
        match self {
            // The same title and artist is the same song, also when it
            // played from the library once and from Spotify another time.
            Kind::Song => vec![(
                format!(
                    "{}\u{1f}{}",
                    entry.title.to_lowercase(),
                    entry.artist.as_deref().unwrap_or_default().to_lowercase()
                ),
                entry.title.clone(),
                entry.artist.clone(),
            )],
            // Every credited artist ("A feat. B" counts for both).
            Kind::Artist => {
                let credit = entry.artist.as_deref().or(entry.album_artist.as_deref());
                let mut names = credit.map(track_artist_names).unwrap_or_default();
                names.dedup_by_key(|n| n.to_lowercase());
                names
                    .into_iter()
                    .map(|name| (name.to_lowercase(), name, None))
                    .collect()
            }
            Kind::Album => entry
                .album
                .iter()
                .filter(|album| !album.trim().is_empty())
                .map(|album| {
                    let artist = entry.album_artist.clone().or_else(|| entry.artist.clone());
                    (
                        format!(
                            "{}\u{1f}{}",
                            album.to_lowercase(),
                            artist.as_deref().unwrap_or_default().to_lowercase()
                        ),
                        album.clone(),
                        artist,
                    )
                })
                .collect(),
            Kind::Genre => entry
                .genre
                .iter()
                .filter(|genre| !genre.trim().is_empty())
                .map(|genre| (genre.trim().to_lowercase(), genre.trim().to_owned(), None))
                .collect(),
        }
    }

    fn key_of(self, ranked: &Ranked) -> String {
        match self {
            Kind::Song | Kind::Album => format!(
                "{}\u{1f}{}",
                ranked.name.to_lowercase(),
                ranked.artist.as_deref().unwrap_or_default().to_lowercase()
            ),
            Kind::Artist | Kind::Genre => ranked.name.to_lowercase(),
        }
    }
}

/// Plays per group, most first (then listening time, then name).
fn tally(entries: &[PlayEntry], kind: Kind) -> Vec<Ranked> {
    let mut groups: HashMap<String, Ranked> = HashMap::new();
    for entry in entries {
        for (key, name, artist) in kind.groups(entry) {
            let ranked = groups.entry(key).or_insert_with(|| Ranked {
                name: name.clone(),
                artist: artist.clone(),
                plays: 0,
                ms_played: 0,
                track_id: entry.track_id,
                trend: None,
            });
            ranked.plays += 1;
            ranked.ms_played += entry.ms_played;
            // Entries come oldest first: the latest names and song win.
            ranked.name = name;
            ranked.artist = artist;
            ranked.track_id = entry.track_id;
        }
    }
    let mut ranked: Vec<Ranked> = groups.into_values().collect();
    ranked.sort_by(|a, b| {
        b.plays
            .cmp(&a.plays)
            .then(b.ms_played.cmp(&a.ms_played))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::StreamInfo;
    use crate::db::tests::{file, rebuild};

    fn play(title: &str, artist: &str, album: &str, genre: Option<&str>) -> PlayEntry {
        PlayEntry {
            played: 0,
            ms_played: 1000,
            track_id: 1,
            title: title.into(),
            artist: Some(artist.into()),
            album: Some(album.into()),
            album_artist: Some(artist.into()),
            genre: genre.map(Into::into),
        }
    }

    #[test]
    fn plays_are_logged_with_what_played() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(&[file("/m/A/B/01.flac", "B", "Local", 1)], 1)
            .unwrap();
        rebuild(&mut db);
        let local: i64 = db
            .conn
            .query_row("SELECT id FROM track", [], |row| row.get(0))
            .unwrap();
        let stream = db
            .stream_track_id(&StreamInfo {
                uri: "spotify:track:s".into(),
                title: "Streamed".into(),
                artist: Some("RM, eAeon".into()),
                album: Some("Indigo".into()),
                duration_ms: 1000,
                ..StreamInfo::default()
            })
            .unwrap();
        db.log_play(local, 10, 90_000).unwrap();
        db.log_play(stream, 20, 1000).unwrap();
        db.log_play(987_654, 30, 1000).unwrap();
        db.log_play(-987_654, 40, 1000).unwrap();
        let log = db.play_log(0, 100).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].title, "Local");
        assert_eq!(log[0].album_artist.as_deref(), Some("Adele"));
        assert_eq!(log[0].genre.as_deref(), Some("Pop"));
        assert_eq!(log[0].ms_played, 90_000);
        assert_eq!(log[1].track_id, stream);
        assert_eq!(log[1].album.as_deref(), Some("Indigo"));
        assert_eq!(db.play_log(15, 100).unwrap().len(), 1);
        assert_eq!(db.first_play().unwrap(), Some(10));
        // Removed songs stay in the statistics; clearing the history
        // doesn't touch them.
        db.delete_tracks(&[std::path::PathBuf::from("/m/A/B/01.flac")])
            .unwrap();
        db.clear_history().unwrap();
        assert_eq!(db.play_log(0, 100).unwrap().len(), 2);
        db.delete_play_log().unwrap();
        assert!(db.play_log(0, 100).unwrap().is_empty());
        assert_eq!(db.first_play().unwrap(), None);
    }

    #[test]
    fn most_played_first_with_the_change_since_before() {
        let current = [
            play("Sorry", "Beyoncé", "Lemonade", Some("Pop")),
            play("Sorry", "Beyoncé", "Lemonade", Some("Pop")),
            play("Wild Flower", "RM (feat. Youjeen)", "Indigo", Some("K-Pop")),
            play("Hold Up", "Beyoncé", "Lemonade", None),
            play("Still Life", "RM, Anderson .Paak", "Indigo", Some("K-Pop")),
        ];
        let previous = [
            play("Wild Flower", "RM (feat. Youjeen)", "Indigo", None),
            play("Wild Flower", "RM (feat. Youjeen)", "Indigo", None),
            play("Sorry", "Beyoncé", "Lemonade", None),
        ];
        let summary = Summary::new(&current, &previous);
        assert_eq!(summary.plays, 5);
        assert_eq!(summary.ms_played, 5000);
        let songs: Vec<(&str, u32)> = summary
            .songs
            .iter()
            .map(|s| (s.name.as_str(), s.plays))
            .collect();
        assert_eq!(songs[0], ("Sorry", 2));
        assert_eq!(summary.songs[0].trend, Some(Trend::Moved(1)));
        let hold_up = summary.songs.iter().find(|s| s.name == "Hold Up").unwrap();
        assert_eq!(hold_up.trend, Some(Trend::New));
        // Each credited artist counts.
        let artists: Vec<(&str, u32)> = summary
            .artists
            .iter()
            .map(|a| (a.name.as_str(), a.plays))
            .collect();
        assert_eq!(artists[0], ("Beyoncé", 3));
        assert_eq!(artists[1], ("RM", 2));
        assert!(artists.contains(&("Youjeen", 1)));
        assert!(artists.contains(&("Anderson .Paak", 1)));
        assert_eq!(summary.albums[0].name, "Lemonade");
        assert_eq!(summary.albums[0].plays, 3);
        assert_eq!(summary.genres[0].name, "K-Pop");
        assert_eq!(summary.genres.len(), 2);
        // Nothing before: no trends.
        assert!(
            Summary::new(&current, &[])
                .songs
                .iter()
                .all(|s| s.trend.is_none())
        );
    }

    #[test]
    fn library_albums_and_artists_by_name() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(&[file("/m/A/B/01.flac", "Twenty Five", "One", 1)], 1)
            .unwrap();
        rebuild(&mut db);
        let (id, cover) = db
            .album_named("twenty five", Some("adele"))
            .unwrap()
            .unwrap();
        assert!(id > 0);
        assert_eq!(cover.as_deref(), Some("eabc"));
        // The only album of that title, by another credit.
        assert!(
            db.album_named("Twenty Five", Some("Someone"))
                .unwrap()
                .is_some()
        );
        assert!(db.album_named("Nineteen", None).unwrap().is_none());
        assert!(db.artist_named("ADELE").unwrap().is_some());
        assert!(db.artist_named("Nobody").unwrap().is_none());
    }
}
