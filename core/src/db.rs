//! The library database: SQLite via rusqlite (CONCEPT.md §3.2, §6.1).
//!
//! The UI and the scanner each open their own connection; WAL mode lets the
//! UI read while a scan writes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params};
use rusqlite_migration::{M, Migrations};

use crate::library::TrackRecord;
use crate::library::artists::{AlbumRole, Aliases, ArtistInfo};
use crate::library::grouping::AlbumInfo;
use crate::library::tags::{Lyrics, TrackTags};
use crate::library::titles::ReleaseType;
use crate::playlist_files::Entry as PlaylistFileEntry;
use crate::queue::{Origin, QueueSnapshot, RepeatMode};

mod spotify;
pub use spotify::{LinkedPlaylist, RemoteEntry, SpotifyPlaylistInfo, SpotifyVersion, StreamInfo};
mod new_releases;
pub use new_releases::{KnownAlbums, NewRelease};
mod spotify_library;
mod stats;
pub use spotify_library::{SpotifyAlbum, SpotifyArtist};
pub use stats::{PlayEntry, Ranked, Summary, Trend};

const MIGRATIONS: &[M<'static>] = &[
    M::up(include_str!("db/001_library.sql")),
    M::up(include_str!("db/002_barcode_and_album_order.sql")),
    M::up(include_str!("db/003_artists.sql")),
    M::up(include_str!("db/004_playlists_search.sql")),
    M::up(include_str!("db/005_play_history.sql")),
    M::up(include_str!("db/006_analysis.sql")),
    M::up(include_str!("db/007_spotify.sql")),
    M::up(include_str!("db/008_streaming.sql")),
    M::up(include_str!("db/009_reread_spotify.sql")),
    M::up(include_str!("db/010_larger_spotify_images.sql")),
    M::up(include_str!("db/011_song_edges.sql")),
    M::up(include_str!("db/012_loudness.sql")),
    M::up(include_str!("db/013_spotify_library.sql")),
    M::up(include_str!("db/014_folder_covers.sql")),
    M::up(include_str!("db/015_declined_suggestions.sql")),
    M::up(include_str!("db/016_indexes.sql")),
    M::up(include_str!("db/017_app_state.sql")),
    M::up(include_str!("db/018_playlist_sort.sql")),
    M::up(include_str!("db/019_portrait_sources.sql")),
    M::up(include_str!("db/020_musicbrainz_isrcs.sql")),
    M::up(include_str!("db/021_ctime.sql")),
    M::up(include_str!("db/022_playlist_search_rowids.sql")),
    M::up(include_str!("db/023_lrclib.sql")),
    M::up(include_str!("db/024_similar_artists.sql")),
    M::up(include_str!("db/025_romanized.sql")),
    M::up(include_str!("db/026_spotify_history.sql")),
    M::up(include_str!("db/027_play_log.sql")),
    M::up(include_str!("db/028_unknown_lengths.sql")),
    M::up(include_str!("db/029_new_releases.sql")),
    M::up(include_str!("db/030_release_artists.sql")),
    M::up(include_str!("db/031_playlist_library_only.sql")),
];

/// Rebuilds the album table with AUTOINCREMENT, once
/// (`db/album_autoincrement.sql`). Needs foreign keys off.
fn album_ids_autoincrement(conn: &mut Connection) -> Result<()> {
    let sql: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'album'",
        [],
        |row| row.get(0),
    )?;
    if sql.contains("AUTOINCREMENT") {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute_batch(include_str!("db/album_autoincrement.sql"))?;
    let broken: i64 = tx.query_row(
        "SELECT count(*) FROM pragma_foreign_key_check WHERE parent = 'album'",
        [],
        |row| row.get(0),
    )?;
    if broken > 0 {
        // Rolled back on drop: the table stays as it was.
        tracing::warn!("Album IDs stay as they are: {broken} foreign keys don't fit");
        return Ok(());
    }
    tx.commit()?;
    Ok(())
}

/// Development builds of 0.6.0 ran a first version of
/// `029_new_releases.sql` without `new_release.seen` (added to the same
/// file the same day): their databases get it here, before migrating on.
/// Don't edit a migration once it has run anywhere; add one.
fn release_seen_column(conn: &Connection) -> Result<()> {
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('new_release')")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let has = |name: &str| columns.iter().any(|c| c == name);
    // Only the table 029 made (030 rebuilds it with `seen`).
    if has("artist_spotify_id") && !has("seen") {
        conn.execute_batch("ALTER TABLE new_release ADD COLUMN seen INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(())
}

/// Separates the values of multi-value tags in one column.
const LIST_SEPARATOR: char = '\u{1f}';

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("cannot create the data folder: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, DbError>;

/// An album as shown in the library.
#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    pub id: i64,
    pub group_key: String,
    pub title: String,
    pub sort_title: String,
    pub edition: Option<String>,
    pub artist_credit: String,
    pub artist_sort: String,
    pub year: Option<i32>,
    pub release_type: ReleaseType,
    pub soundtrack: bool,
    pub compilation: bool,
    pub track_count: u32,
    pub duration_ms: u64,
    pub cover_key: Option<String>,
    pub added: i64,
}

/// A track with what the player needs to show about its album.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub record: TrackRecord,
    pub album_id: Option<i64>,
    pub album_title: Option<String>,
    pub album_artist: Option<String>,
    pub album_cover_key: Option<String>,
    /// The disc shown in the album panel (usually the tagged one).
    pub album_disc: Option<u32>,
}

/// An album artist as shown in the library (CONCEPT.md §3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Artist {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub sort_name: String,
    /// From the tags, else from the online lookup.
    pub mbid: Option<String>,
    /// Albums credited to the artist (not collaborations).
    pub album_count: u32,
    pub track_count: u32,
    /// The portrait to show: the custom one, else the automatic one.
    pub portrait_key: Option<String>,
    /// The cover of the artist's newest album, for the fallback avatar.
    pub cover_key: Option<String>,
    /// When the artist came into the library: the first of their albums
    /// to be added (Unix seconds; 0 without albums of their own).
    pub added: i64,
    pub meta: ArtistMeta,
}

/// What the online lookup found (CONCEPT.md §8).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtistMeta {
    /// "done", "not-found" or "ambiguous"; `None` until looked up.
    pub lookup_state: Option<String>,
    /// Unix seconds of the last lookup.
    pub looked_up: Option<i64>,
    pub lookup_mbid: Option<String>,
    pub wikidata_id: Option<String>,
    pub spotify_id: Option<String>,
    pub wikipedia_url: Option<String>,
    pub website: Option<String>,
    pub bio: Option<String>,
    pub portrait_key: Option<String>,
    /// "local", "fanart.tv" or "wikimedia".
    pub portrait_source: Option<String>,
    pub portrait_attribution: Option<String>,
    pub custom_portrait_key: Option<String>,
}

/// A playlist: local, a Spotify mirror, or local and published to Spotify.
#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    /// 'local' or 'spotify' (came from Spotify).
    pub source: String,
    /// Set when the playlist is linked to a Spotify playlist (§9.6).
    pub spotify_id: Option<String>,
    pub spotify_owner: Option<String>,
    /// Owned or collaborative on Spotify; always true for local ones.
    pub editable: bool,
    /// False when Spotify doesn't show the entries (§9.5).
    pub readable: bool,
    pub spotify_cover_key: Option<String>,
    /// Changes not sent to Spotify yet.
    pub pending_changes: u32,
    pub custom_cover_key: Option<String>,
    pub created: i64,
    pub modified: i64,
    pub item_count: u32,
    /// Entries that play from the library.
    pub local_count: u32,
    pub duration_ms: u64,
    /// Up to four album covers of its entries, for the mosaic.
    pub cover_keys: Vec<String>,
}

/// A playlist entry and, when it plays from the library, its track.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistItem {
    pub id: i64,
    pub position: u32,
    pub track: Option<Track>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    /// 'local' | 'unavailable' for library entries; 'matched' |
    /// 'user_matched' | 'spotify_only' | 'user_pinned_spotify' for Spotify
    /// entries (§9.5).
    pub match_state: String,
    pub spotify_uri: Option<String>,
    /// A library song that is probably this Spotify entry.
    pub suggested_track: Option<Track>,
    /// The Spotify track (JSON) that is probably this library song.
    pub suggested_spotify: Option<String>,
    /// The Spotify album image, for entries not in the library.
    pub image_url: Option<String>,
    /// When it was added (Unix seconds), if known.
    pub added: Option<i64>,
}

/// What a search found, best first per kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub artists: Vec<i64>,
    pub albums: Vec<i64>,
    pub tracks: Vec<i64>,
    pub playlists: Vec<i64>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.artists.is_empty()
            && self.albums.is_empty()
            && self.tracks.is_empty()
            && self.playlists.is_empty()
    }
}

/// A file found by the scanner, with its stat data and tags.
/// Lyrics LRCLIB gave for a Spotify song, or that it had none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OnlineLyricsRow {
    pub checked: i64,
    pub found: bool,
    pub synced: Option<String>,
    pub plain: Option<String>,
    pub instrumental: bool,
}

#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub mtime: i64,
    pub size: i64,
    pub tags: TrackTags,
    pub cover_key: Option<String>,
}

pub struct Database {
    conn: Connection,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("path", &self.conn.path())
            .finish()
    }
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::setup(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // Foreign keys only after migrating: rebuilding a table drops the
        // old one, which would otherwise empty the tables referring to it.
        conn.pragma_update(None, "foreign_keys", false)?;
        release_seen_column(&conn)?;
        Migrations::from_slice(MIGRATIONS).to_latest(&mut conn)?;
        album_ids_autoincrement(&mut conn)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(Self { conn })
    }

    // Scanner side ----------------------------------------------------------

    /// Modification time and size of every known file.
    pub fn file_stamps(&self) -> Result<HashMap<PathBuf, (i64, i64)>> {
        let mut stmt = self.conn.prepare("SELECT path, mtime, size FROM track")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                PathBuf::from(row.get::<_, String>(0)?),
                (row.get(1)?, row.get(2)?),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The files' change times as last seen (`None`: not yet recorded).
    pub fn file_ctimes(&self) -> Result<HashMap<PathBuf, Option<i64>>> {
        let mut stmt = self.conn.prepare("SELECT path, ctime FROM track")?;
        let rows = stmt.query_map([], |row| {
            Ok((PathBuf::from(row.get::<_, String>(0)?), row.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_ctimes(&mut self, ctimes: &[(PathBuf, i64)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached("UPDATE track SET ctime = ?2 WHERE path = ?1")?;
            for (path, ctime) in ctimes {
                stmt.execute(params![path_str(path), ctime])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The audio fingerprints (`analysis::content_key`) known for these
    /// files.
    pub fn content_keys(&self, paths: &[PathBuf]) -> Result<HashMap<PathBuf, String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT content_key FROM track WHERE path = ?1")?;
        let mut keys = HashMap::new();
        for path in paths {
            let key: Option<String> = stmt
                .query_row([path_str(path)], |row| row.get(0))
                .optional()?
                .flatten();
            if let Some(key) = key {
                keys.insert(path.clone(), key);
            }
        }
        Ok(keys)
    }

    /// Files that moved: their songs keep their ID (play history, queue,
    /// playlists, Spotify matches, when they were added).
    pub fn move_tracks(&mut self, moves: &[(PathBuf, PathBuf)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached("UPDATE track SET path = ?2 WHERE path = ?1")?;
            for (from, to) in moves {
                stmt.execute(params![path_str(from), path_str(to)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Inserts or updates tracks by path. Existing tracks keep their ID and
    /// the time they were added.
    pub fn upsert_tracks(&mut self, files: &[ScannedFile], now: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(UPSERT_TRACK)?;
            for file in files {
                let t = &file.tags;
                stmt.execute(params![
                    path_str(&file.path),
                    file.mtime,
                    file.size,
                    now,
                    t.title,
                    t.artist,
                    t.album,
                    t.album_artist,
                    join_list(&t.album_artists),
                    t.album_artist_sort,
                    t.disc,
                    t.number,
                    t.year,
                    join_list(&t.genres),
                    t.composer,
                    t.mb_album_id,
                    t.mb_release_group_id,
                    t.mb_track_id,
                    join_list(&t.mb_album_artist_ids),
                    t.release_comment,
                    t.release_type,
                    t.compilation,
                    t.isrc,
                    t.bpm,
                    t.lyrics.as_ref().map(|l| l.synced),
                    t.lyrics.as_ref().map(|l| &l.excerpt),
                    t.replaygain_track,
                    t.replaygain_album,
                    t.duration_ms as i64,
                    t.format,
                    t.bitrate,
                    t.sample_rate,
                    t.bit_depth,
                    t.channels,
                    file.cover_key,
                    t.barcode,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_tracks(&mut self, paths: &[PathBuf]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached("DELETE FROM track WHERE path = ?1")?;
            for path in paths {
                stmt.execute([path_str(path)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Cached online lyrics of a Spotify song: (checked, found, synced,
    /// plain, instrumental).
    pub fn online_lyrics(&self, uri: &str) -> Result<Option<OnlineLyricsRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT checked, found, synced, plain, instrumental FROM online_lyrics
                 WHERE uri = ?1",
                [uri],
                |row| {
                    Ok(OnlineLyricsRow {
                        checked: row.get(0)?,
                        found: row.get(1)?,
                        synced: row.get(2)?,
                        plain: row.get(3)?,
                        instrumental: row.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_online_lyrics(&mut self, uri: &str, row: &OnlineLyricsRow) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO online_lyrics (uri, checked, found, synced, plain, instrumental)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                uri,
                row.checked,
                row.found,
                row.synced,
                row.plain,
                row.instrumental
            ],
        )?;
        Ok(())
    }

    /// A cached romanization: `Some(None)` when lyrembed had nothing to
    /// do, if it said so since `misses_since` (a backend installed later
    /// may romanize it after all).
    pub fn romanized(&self, key: &str, misses_since: i64) -> Result<Option<Option<String>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT lyrics FROM romanized
                 WHERE key = ?1 AND (lyrics IS NOT NULL OR checked >= ?2)",
                params![key, misses_since],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_romanized(&mut self, key: &str, lyrics: Option<&str>, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO romanized (key, lyrics, checked) VALUES (?1, ?2, ?3)",
            params![key, lyrics, now],
        )?;
        Ok(())
    }

    /// The artist portraits in use (found, online, or chosen by the user).
    pub fn portrait_keys_in_use(&self) -> Result<HashSet<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT portrait_key FROM artist UNION SELECT custom_portrait_key FROM artist",
        )?;
        let keys = stmt
            .query_map([], |row| row.get::<_, Option<String>>(0))?
            .filter_map(|key| key.ok().flatten())
            .collect();
        Ok(keys)
    }

    /// Every cover key something refers to: songs, albums, playlists, and
    /// the Spotify images of entries, songs to stream, saved albums,
    /// followed artists and their new releases (their keys derive from the
    /// image address).
    pub fn cover_keys_in_use(&self) -> Result<HashSet<String>> {
        let mut keys = HashSet::new();
        for sql in [
            "SELECT cover_key FROM track",
            "SELECT cover_key FROM album",
            "SELECT custom_cover_key FROM playlist",
            "SELECT spotify_cover_key FROM playlist",
            "SELECT cover_key FROM stream_track",
        ] {
            let mut stmt = self.conn.prepare(sql)?;
            for key in stmt.query_map([], |row| row.get::<_, Option<String>>(0))? {
                keys.extend(key?);
            }
        }
        for sql in [
            "SELECT spotify_image_url FROM playlist",
            "SELECT image_url FROM playlist_item",
            "SELECT image_url FROM stream_track",
            "SELECT image_url FROM spotify_album",
            "SELECT image_url FROM spotify_artist",
            "SELECT image_url FROM new_release",
        ] {
            let mut stmt = self.conn.prepare(sql)?;
            for url in stmt.query_map([], |row| row.get::<_, Option<String>>(0))? {
                if let Some(url) = url? {
                    keys.insert(crate::library::covers::cover_key('s', url.as_bytes()));
                }
            }
        }
        Ok(keys)
    }

    pub fn app_state(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM app_state WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn set_app_state(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO app_state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Folder cover images as last seen: folder → (image, mtime, size).
    pub fn folder_cover_stamps(&self) -> Result<HashMap<PathBuf, (PathBuf, i64, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT folder, image, mtime, size FROM folder_cover")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                PathBuf::from(row.get::<_, String>(0)?),
                (
                    PathBuf::from(row.get::<_, String>(1)?),
                    row.get(2)?,
                    row.get(3)?,
                ),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The folder cover images now; folders not listed have none.
    pub fn replace_folder_cover_stamps(
        &mut self,
        stamps: &HashMap<PathBuf, (PathBuf, i64, i64)>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM folder_cover", [])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO folder_cover (folder, image, mtime, size) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (folder, (image, mtime, size)) in stamps {
                insert.execute(params![
                    folder.to_string_lossy(),
                    image.to_string_lossy(),
                    mtime,
                    size
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// New cover keys for songs (folder covers that changed).
    pub fn set_track_cover_keys(&mut self, keys: &[(i64, Option<String>)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut update = tx.prepare("UPDATE track SET cover_key = ?2 WHERE id = ?1")?;
            for (id, key) in keys {
                update.execute(params![id, key])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn all_tracks(&self) -> Result<Vec<TrackRecord>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {TRACK_COLUMNS} FROM track t"))?;
        let rows = stmt.query_map([], track_record)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replaces the album and artist tables with freshly grouped data.
    /// Albums and artists keep their ID while their identity key stays the
    /// same; artists keep their online metadata.
    /// Returns how many albums, song links and artists it wrote.
    pub fn replace_library(
        &mut self,
        albums: &[AlbumInfo],
        artists: &[ArtistInfo],
    ) -> Result<(usize, usize, usize)> {
        // Everything is grouped afresh, but only what differs is written:
        // after a scan that found one new CD, the transaction is short (the
        // main thread may be waiting to write, e.g. the play history).
        let tx = self.conn.transaction()?;
        let mut album_ids: HashMap<&str, i64> = HashMap::new();
        let mut artist_ids = HashSet::new();
        let mut written = (0usize, 0usize, 0usize);
        {
            // What's there: albums by key (id and stored values), the
            // songs' album links, the artists by key.
            let mut stored_albums: HashMap<String, (i64, Vec<Value>)> = HashMap::new();
            {
                let mut stmt = tx.prepare(&format!("SELECT id, key, {ALBUM_STORED} FROM album"))?;
                let mut rows = stmt.query([])?;
                while let Some(row) = rows.next()? {
                    let values = (2..2 + ALBUM_STORED_COUNT)
                        .map(|i| row.get::<_, Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    stored_albums.insert(row.get(1)?, (row.get(0)?, values));
                }
            }
            let stored_links: HashMap<i64, AlbumLink> = {
                let mut stmt =
                    tx.prepare("SELECT id, album_id, album_position, album_disc FROM track")?;
                stmt.query_map([], |row| {
                    Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?)))
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            let mut upsert = tx.prepare_cached(UPSERT_ALBUM)?;
            let mut link = tx.prepare_cached(
                "UPDATE track SET album_id = ?1, album_position = ?3, album_disc = ?4 WHERE id = ?2",
            )?;
            for album in albums {
                let values = album_values(album)?;
                let id = match stored_albums.get(&album.key) {
                    Some((id, stored)) if *stored == values => *id,
                    _ => {
                        written.0 += 1;
                        let mut params: Vec<Value> = vec![Value::Text(album.key.clone())];
                        params.extend(values);
                        upsert.query_row(rusqlite::params_from_iter(params), |row| row.get(0))?
                    }
                };
                album_ids.insert(&album.key, id);
                for (position, track_id) in album.track_ids.iter().enumerate() {
                    let disc = album.track_discs.get(position).copied().map(i64::from);
                    let wanted = (Some(id), Some(position as i64), disc);
                    if stored_links.get(track_id) != Some(&wanted) {
                        written.1 += 1;
                        link.execute(params![id, track_id, position as i64, disc])?;
                    }
                }
            }

            let stored_artists: HashMap<String, StoredArtist> = {
                let mut stmt = tx.prepare("SELECT key, id, name, sort_name, mbid FROM artist")?;
                stmt.query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?),
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            let mut upsert_artist = tx.prepare_cached(UPSERT_ARTIST)?;
            // The wanted links: album → (artist, role, position), song → artists.
            let mut album_links: HashMap<i64, Vec<(i64, String, i64)>> = HashMap::new();
            let mut track_links: HashMap<i64, Vec<i64>> = HashMap::new();
            for artist in artists {
                let id = match stored_artists.get(&artist.key) {
                    Some((id, name, sort, mbid))
                        if name.as_deref() == Some(artist.name.as_str())
                            && sort.as_deref() == Some(artist.sort_name.as_str())
                            && *mbid == artist.mbid =>
                    {
                        *id
                    }
                    _ => {
                        written.2 += 1;
                        upsert_artist.query_row(
                            params![artist.key, artist.name, artist.sort_name, artist.mbid],
                            |row| row.get(0),
                        )?
                    }
                };
                artist_ids.insert(id);
                for (album_key, role, position) in &artist.albums {
                    if let Some(album_id) = album_ids.get(album_key.as_str()) {
                        album_links.entry(*album_id).or_default().push((
                            id,
                            role.as_str().to_owned(),
                            *position as i64,
                        ));
                    }
                }
                for track_id in &artist.appears_on {
                    track_links.entry(*track_id).or_default().push(id);
                }
            }
            sync_links(
                &tx,
                "SELECT album_id, artist_id, role, position FROM album_artist",
                |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?))),
                album_links,
                "DELETE FROM album_artist WHERE album_id = ?1",
                |tx, album_id, (artist_id, role, position): &(i64, String, i64)| {
                    tx.prepare_cached(
                        "INSERT OR IGNORE INTO album_artist (album_id, artist_id, role, position)
                         VALUES (?1, ?2, ?3, ?4)",
                    )?
                    .execute(params![album_id, artist_id, role, position])
                    .map(drop)
                },
            )?;
            sync_links(
                &tx,
                "SELECT track_id, artist_id FROM track_artist",
                |row| Ok((row.get(0)?, row.get(1)?)),
                track_links,
                "DELETE FROM track_artist WHERE track_id = ?1",
                |tx, track_id, artist_id: &i64| {
                    tx.prepare_cached(
                        "INSERT OR IGNORE INTO track_artist (track_id, artist_id) VALUES (?1, ?2)",
                    )?
                    .execute(params![track_id, artist_id])
                    .map(drop)
                },
            )?;
        }
        let kept: HashSet<i64> = album_ids.into_values().collect();
        delete_except(&tx, "album", &kept)?;
        carry_artist_data(&tx, &artist_ids)?;
        delete_except(&tx, "artist", &artist_ids)?;
        tx.commit()?;
        tracing::debug!(
            "Library written: {} albums, {} song links, {} artists changed",
            written.0,
            written.1,
            written.2
        );
        Ok(written)
    }

    /// For each cover key, a track that has that cover.
    pub fn album_cover_sources(&self) -> Result<Vec<(String, PathBuf)>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.cover_key, MIN(t.path) FROM album a JOIN track t
               ON t.album_id = a.id AND t.cover_key = a.cover_key
             WHERE a.cover_key IS NOT NULL GROUP BY a.cover_key",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // UI side ---------------------------------------------------------------

    pub fn albums(&self) -> Result<Vec<Album>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ALBUM_COLUMNS} FROM album ORDER BY artist_sort, year, sort_title, id"
        ))?;
        let rows = stmt.query_map([], album)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn album(&self, id: i64) -> Result<Option<Album>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ALBUM_COLUMNS} FROM album WHERE id = ?1"),
                [id],
                album,
            )
            .optional()?)
    }

    /// The album's tracks in play order.
    pub fn album_tracks(&self, album_id: i64) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TRACK_COLUMNS}, {TRACK_ALBUM_COLUMNS} FROM track t
               LEFT JOIN album a ON a.id = t.album_id
             WHERE t.album_id = ?1
             ORDER BY t.album_position, COALESCE(t.disc, 1), COALESCE(t.number, 1000000),
               lower(t.title), t.path"
        ))?;
        let rows = stmt.query_map([album_id], track)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Library songs by ID; negative IDs are Spotify songs to stream
    /// (`stream_track`).
    pub fn tracks(&self, ids: &[i64]) -> Result<HashMap<i64, Track>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {TRACK_COLUMNS}, {TRACK_ALBUM_COLUMNS} FROM track t
               LEFT JOIN album a ON a.id = t.album_id
             WHERE t.id = ?1"
        ))?;
        let mut tracks = HashMap::new();
        for id in ids {
            let found = if *id < 0 {
                self.stream_track(*id)?
            } else {
                stmt.query_row([id], track).optional()?
            };
            if let Some(track) = found {
                tracks.insert(*id, track);
            }
        }
        Ok(tracks)
    }

    // Artists ---------------------------------------------------------------

    pub fn artists(&self) -> Result<Vec<Artist>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ARTIST_COLUMNS} FROM artist ar ORDER BY ar.sort_name, ar.id"
        ))?;
        let rows = stmt.query_map([], artist)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn artist(&self, id: i64) -> Result<Option<Artist>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ARTIST_COLUMNS} FROM artist ar WHERE ar.id = ?1"),
                [id],
                artist,
            )
            .optional()?)
    }

    /// The artist's albums and collaborations, oldest first.
    pub fn artist_albums(&self, artist_id: i64) -> Result<Vec<(Album, AlbumRole)>> {
        let columns = ALBUM_COLUMNS
            .split(',')
            .map(|c| format!("a.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {columns}, aa.role FROM album_artist aa JOIN album a ON a.id = aa.album_id
             WHERE aa.artist_id = ?1 ORDER BY a.year, a.sort_title, a.id"
        ))?;
        let rows = stmt.query_map([artist_id], |row| {
            Ok((
                album(row)?,
                AlbumRole::from_str_lossy(&row.get::<_, String>(15)?),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Tracks on other artists' albums ("Appears on"), by album.
    pub fn artist_appears_on(&self, artist_id: i64) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TRACK_COLUMNS}, {TRACK_ALBUM_COLUMNS} FROM track_artist ta
               JOIN track t ON t.id = ta.track_id
               LEFT JOIN album a ON a.id = t.album_id
             WHERE ta.artist_id = ?1
             ORDER BY a.year, a.sort_title, t.album_position"
        ))?;
        let rows = stmt.query_map([artist_id], track)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The album artists of an album, in credit order: (id, name).
    pub fn album_artists(&self, album_id: i64) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT ar.id, ar.name FROM album_artist aa JOIN artist ar ON ar.id = aa.artist_id
             WHERE aa.album_id = ?1 AND aa.role = 'credit' ORDER BY aa.position",
        )?;
        let rows = stmt.query_map([album_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Release MBIDs of the artist's albums, for the MusicBrainz lookup.
    pub fn artist_release_mbids(&self, artist_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT a.mb_album_id FROM album_artist aa JOIN album a ON a.id = aa.album_id
             WHERE aa.artist_id = ?1 AND aa.role = 'credit' AND a.mb_album_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([artist_id], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Folders of the artist's albums' tracks, for local portraits.
    pub fn artist_track_paths(&self, artist_id: i64) -> Result<Vec<PathBuf>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.path FROM album_artist aa JOIN track t ON t.album_id = aa.album_id
             WHERE aa.artist_id = ?1 AND aa.role = 'credit'",
        )?;
        let rows = stmt.query_map([artist_id], |row| {
            Ok(PathBuf::from(row.get::<_, String>(0)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Stores what the online lookup found. The custom portrait is kept.
    pub fn set_artist_meta(&mut self, artist_id: i64, meta: &ArtistMeta) -> Result<()> {
        self.conn.execute(
            "UPDATE artist SET lookup_state = ?2, looked_up = ?3, lookup_mbid = ?4,
               wikidata_id = ?5, spotify_id = ?6, wikipedia_url = ?7, website = ?8, bio = ?9,
               portrait_key = ?10, portrait_source = ?11, portrait_attribution = ?12
             WHERE id = ?1",
            params![
                artist_id,
                meta.lookup_state,
                meta.looked_up,
                meta.lookup_mbid,
                meta.wikidata_id,
                meta.spotify_id,
                meta.wikipedia_url,
                meta.website,
                meta.bio,
                meta.portrait_key,
                meta.portrait_source,
                meta.portrait_attribution,
            ],
        )?;
        Ok(())
    }

    /// Sets or clears (`None`: back to automatic) the user's portrait.
    pub fn set_custom_portrait(&mut self, artist_id: i64, key: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE artist SET custom_portrait_key = ?2 WHERE id = ?1",
            params![artist_id, key],
        )?;
        Ok(())
    }

    /// Forgets the lookup result, so the next lookup starts over.
    pub fn reset_artist_lookup(&mut self, artist_id: i64, mbid: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE artist SET lookup_state = NULL, looked_up = NULL, lookup_mbid = ?2 WHERE id = ?1",
            params![artist_id, mbid],
        )?;
        Ok(())
    }

    // Playlists (CONCEPT.md §9.5) ------------------------------------------

    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PLAYLIST_COLUMNS} FROM playlist p ORDER BY lower(p.name), p.id"
        ))?;
        let rows = stmt.query_map([], playlist)?;
        let mut playlists: Vec<Playlist> = rows.collect::<rusqlite::Result<_>>()?;
        for p in &mut playlists {
            p.cover_keys = self.playlist_cover_keys(p.id)?;
        }
        Ok(playlists)
    }

    pub fn playlist(&self, id: i64) -> Result<Option<Playlist>> {
        let found = self
            .conn
            .query_row(
                &format!("SELECT {PLAYLIST_COLUMNS} FROM playlist p WHERE p.id = ?1"),
                [id],
                playlist,
            )
            .optional()?;
        let Some(mut playlist) = found else {
            return Ok(None);
        };
        playlist.cover_keys = self.playlist_cover_keys(id)?;
        Ok(Some(playlist))
    }

    fn playlist_cover_keys(&self, id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.cover_key FROM playlist_item i
               JOIN track t ON t.id = i.local_track_id
               JOIN album a ON a.id = t.album_id
             WHERE i.playlist_id = ?1 AND a.cover_key IS NOT NULL
             GROUP BY a.cover_key ORDER BY MIN(i.position) LIMIT 4",
        )?;
        let rows = stmt.query_map([id], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_playlist(&mut self, name: &str, now: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO playlist (name, created, modified) VALUES (?1, ?2, ?2)",
            params![name, now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.index_playlist(id, name)?;
        Ok(id)
    }

    pub fn rename_playlist(&mut self, id: i64, name: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist SET name = ?2, modified = ?3 WHERE id = ?1",
            params![id, name, now],
        )?;
        self.index_playlist(id, name)
    }

    pub fn delete_playlist(&mut self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM playlist WHERE id = ?1", [id])?;
        self.conn.execute(
            "DELETE FROM search WHERE rowid = ?1",
            [playlist_search_row(id)],
        )?;
        Ok(())
    }

    pub fn set_playlist_cover(&mut self, id: i64, key: Option<&str>, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist SET custom_cover_key = ?2, modified = ?3 WHERE id = ?1",
            params![id, key, now],
        )?;
        Ok(())
    }

    /// How a playlist is listed, if chosen for it: ("custom" | "title" |
    /// "artist" | "album" | "added" | "duration", reversed). `None`: never
    /// chosen (the default from Preferences applies).
    ///
    /// Stored without a new column, so older versions still open the
    /// database: the column's default ('custom', not reversed) means "not
    /// chosen", and a chosen own order is stored as 'own' (which older
    /// versions list in the playlist's order too).
    pub fn playlist_sort(&self, id: i64) -> Result<Option<(String, bool)>> {
        let stored: Option<(String, bool)> = self
            .conn
            .query_row(
                "SELECT sort_by, sort_reversed FROM playlist WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(match stored {
            None => None,
            Some((by, false)) if by == "custom" => None,
            Some((by, reversed)) if by == "own" => Some(("custom".to_owned(), reversed)),
            Some(sort) => Some(sort),
        })
    }

    /// Chooses how the playlist is listed; `None` goes back to the default.
    pub fn set_playlist_sort(&mut self, id: i64, sort: Option<(&str, bool)>) -> Result<()> {
        let (by, reversed) = match sort {
            None => ("custom", false),
            Some(("custom", reversed)) => ("own", reversed),
            Some(sort) => sort,
        };
        self.conn.execute(
            "UPDATE playlist SET sort_by = ?2, sort_reversed = ?3 WHERE id = ?1",
            params![id, by, reversed],
        )?;
        Ok(())
    }

    /// The playlist shows only the songs that play from the library.
    pub fn playlist_library_only(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT library_only FROM playlist WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn set_playlist_library_only(&mut self, id: i64, only: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist SET library_only = ?2 WHERE id = ?1",
            params![id, only],
        )?;
        Ok(())
    }

    pub fn playlist_items(&self, id: i64) -> Result<Vec<PlaylistItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, position, local_track_id, title, artist, album, duration_ms, match_state,
                    spotify_uri, suggested_track_id, suggested_spotify, image_url, added
             FROM playlist_item WHERE playlist_id = ?1 ORDER BY position",
        )?;
        let rows: Vec<(PlaylistItem, Option<i64>, Option<i64>)> = stmt
            .query_map([id], |row| {
                Ok((
                    PlaylistItem {
                        id: row.get(0)?,
                        position: row.get(1)?,
                        track: None,
                        title: row.get(3)?,
                        artist: row.get(4)?,
                        album: row.get(5)?,
                        duration_ms: row.get::<_, Option<i64>>(6)?.map(|d| d.max(0) as u64),
                        match_state: row.get(7)?,
                        spotify_uri: row.get(8)?,
                        suggested_track: None,
                        suggested_spotify: row.get(10)?,
                        image_url: row.get(11)?,
                        added: row.get(12)?,
                    },
                    row.get(2)?,
                    row.get(9)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let ids: Vec<i64> = rows
            .iter()
            .flat_map(|(_, t, s)| [*t, *s])
            .flatten()
            .collect();
        let tracks = self.tracks(&ids)?;
        Ok(rows
            .into_iter()
            .map(|(mut item, track_id, suggested)| {
                item.track = track_id.and_then(|id| tracks.get(&id).cloned());
                item.suggested_track = suggested.and_then(|id| tracks.get(&id).cloned());
                item
            })
            .collect())
    }

    /// Appends library tracks (at `position`, or the end).
    pub fn add_to_playlist(
        &mut self,
        id: i64,
        track_ids: &[i64],
        position: Option<u32>,
        now: i64,
    ) -> Result<()> {
        let tracks = self.tracks(track_ids)?;
        let tx = self.conn.transaction()?;
        let count: u32 = tx.query_row(
            "SELECT COUNT(*) FROM playlist_item WHERE playlist_id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let at = position.unwrap_or(count).min(count);
        // Spotify songs (negative IDs) go in with add_streams_to_playlist;
        // songs a scan removed meanwhile don't go in. Only the rest make
        // room, or the positions would have gaps.
        let adding: Vec<_> = track_ids
            .iter()
            .filter(|track_id| **track_id > 0)
            .filter_map(|track_id| tracks.get(track_id))
            .collect();
        tx.execute(
            "UPDATE playlist_item SET position = position + ?3
             WHERE playlist_id = ?1 AND position >= ?2",
            params![id, at, adding.len() as i64],
        )?;
        {
            let mut insert = tx.prepare_cached(INSERT_ITEM)?;
            for (offset, track) in adding.into_iter().enumerate() {
                let tags = &track.record.tags;
                insert.execute(params![
                    id,
                    at + offset as u32,
                    track.record.id,
                    tags.isrc,
                    tags.title,
                    tags.artist.clone().or(track.album_artist.clone()),
                    track.album_title,
                    tags.duration_ms as i64,
                    path_str(&track.record.path),
                    tags.mb_track_id,
                    "local",
                ])?;
            }
        }
        tx.execute(
            "UPDATE playlist SET modified = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Adds entries read from a playlist file, matched to library tracks
    /// where possible. Returns how many matched.
    pub fn import_entries(
        &mut self,
        id: i64,
        entries: &[PlaylistFileEntry],
        now: i64,
    ) -> Result<usize> {
        let mut matched = 0;
        let tx = self.conn.transaction()?;
        {
            let start: u32 = tx.query_row(
                "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_item WHERE playlist_id = ?1",
                [id],
                |row| row.get(0),
            )?;
            let mut insert = tx.prepare_cached(INSERT_ITEM)?;
            for (offset, entry) in entries.iter().enumerate() {
                let snapshot = Snapshot {
                    path: entry
                        .location
                        .as_deref()
                        .and_then(Path::to_str)
                        .map(str::to_owned),
                    mb_track_id: None,
                    isrc: None,
                    title: entry.title.clone(),
                    artist: entry.artist.clone(),
                    duration_ms: entry.duration_ms,
                };
                let track_id = find_track(&tx, &snapshot)?;
                matched += usize::from(track_id.is_some());
                insert.execute(params![
                    id,
                    start + offset as u32,
                    track_id,
                    Option::<String>::None,
                    entry.title,
                    entry.artist,
                    entry.album,
                    entry.duration_ms.map(|d| d as i64),
                    snapshot.path,
                    Option::<String>::None,
                    if track_id.is_some() {
                        "local"
                    } else {
                        "unavailable"
                    },
                ])?;
            }
        }
        tx.execute(
            "UPDATE playlist SET modified = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        tx.commit()?;
        // Imported entries get their snapshot completed from the tracks.
        self.relink_playlists()?;
        Ok(matched)
    }

    pub fn remove_playlist_item(&mut self, playlist_id: i64, item_id: i64, now: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM playlist_item WHERE id = ?1", [item_id])?;
        self.renumber(playlist_id, now)
    }

    /// Moves an entry to another position (drag & drop).
    pub fn move_playlist_item(
        &mut self,
        playlist_id: i64,
        from: u32,
        to: u32,
        now: i64,
    ) -> Result<()> {
        let mut ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM playlist_item WHERE playlist_id = ?1 ORDER BY position")?;
            stmt.query_map([playlist_id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let (from, to) = (from as usize, to as usize);
        if from >= ids.len() || to >= ids.len() {
            return Ok(());
        }
        let id = ids.remove(from);
        ids.insert(to, id);
        let tx = self.conn.transaction()?;
        {
            let mut update =
                tx.prepare_cached("UPDATE playlist_item SET position = ?2 WHERE id = ?1")?;
            for (position, id) in ids.iter().enumerate() {
                update.execute(params![id, position as i64])?;
            }
        }
        tx.execute(
            "UPDATE playlist SET modified = ?2 WHERE id = ?1",
            params![playlist_id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn renumber(&mut self, playlist_id: i64, now: i64) -> Result<()> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM playlist_item WHERE playlist_id = ?1",
            [playlist_id],
            |row| row.get(0),
        )?;
        if count > 0 {
            let order: Vec<i64> = {
                let mut stmt = self.conn.prepare(
                    "SELECT id FROM playlist_item WHERE playlist_id = ?1 ORDER BY position",
                )?;
                stmt.query_map([playlist_id], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let tx = self.conn.transaction()?;
            for (position, id) in order.iter().enumerate() {
                tx.execute(
                    "UPDATE playlist_item SET position = ?2 WHERE id = ?1",
                    params![id, position as i64],
                )?;
            }
            tx.commit()?;
        }
        self.conn.execute(
            "UPDATE playlist SET modified = ?2 WHERE id = ?1",
            params![playlist_id, now],
        )?;
        Ok(())
    }

    /// After library changes: entries whose track is gone are matched
    /// again (path, MusicBrainz ID, ISRC, then title + artist + duration);
    /// linked entries get their snapshot refreshed.
    pub fn relink_playlists(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE playlist_item SET
               path = (SELECT t.path FROM track t WHERE t.id = local_track_id),
               mb_track_id = COALESCE((SELECT t.mb_track_id FROM track t WHERE t.id = local_track_id), mb_track_id),
               isrc = COALESCE((SELECT t.isrc FROM track t WHERE t.id = local_track_id), isrc),
               match_state = CASE WHEN spotify_uri IS NULL THEN 'local' ELSE match_state END
             WHERE local_track_id IS NOT NULL",
            [],
        )?;
        let orphans: Vec<(i64, Snapshot)> = {
            let mut stmt = tx.prepare(
                "SELECT id, path, mb_track_id, isrc, title, artist, duration_ms FROM playlist_item
                 WHERE local_track_id IS NULL AND spotify_uri IS NULL",
            )?;
            stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    Snapshot {
                        path: row.get(1)?,
                        mb_track_id: row.get(2)?,
                        isrc: row.get(3)?,
                        title: row.get(4)?,
                        artist: row.get(5)?,
                        duration_ms: row.get::<_, Option<i64>>(6)?.map(|d| d.max(0) as u64),
                    },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        for (item, snapshot) in orphans {
            let track = find_track(&tx, &snapshot)?;
            tx.execute(
                "UPDATE playlist_item SET local_track_id = ?2, match_state = ?3,
                   path = COALESCE((SELECT path FROM track WHERE id = ?2), path)
                 WHERE id = ?1",
                params![
                    item,
                    track,
                    if track.is_some() {
                        "local"
                    } else {
                        "unavailable"
                    }
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // Search (FTS5, §4.1) ---------------------------------------------------

    /// Rebuilds the search index after a scan. Artist and album-artist
    /// names include their lyrembed alias spellings.
    pub fn rebuild_search(&mut self, aliases: &Aliases) -> Result<usize> {
        // The rows as they should be, compared with what's there: only
        // changed ones are written (the index is rewritten after every scan
        // that changed something).
        let tx = self.conn.transaction()?;
        let mut wanted: HashMap<(&'static str, i64), String> = HashMap::new();
        {
            let mut stmt = tx.prepare("SELECT id, name FROM artist")?;
            for row in stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })? {
                let (id, name) = row?;
                wanted.insert(("artist", id), aliases.spellings(&name).join(" · "));
            }
            let mut stmt = tx.prepare("SELECT id, title, edition, artist_credit FROM album")?;
            let albums = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            for row in albums {
                let (id, title, edition, credit) = row?;
                let text = format!(
                    "{title} · {} · {}",
                    edition.unwrap_or_default(),
                    aliases.spellings(&credit).join(" · ")
                );
                wanted.insert(("album", id), text);
            }
            let mut stmt = tx.prepare(
                "SELECT t.id, COALESCE(t.title, '') || ' · ' || COALESCE(t.artist, '') || ' · '
                   || COALESCE(a.title, '') || ' · ' || COALESCE(t.composer, '')
                 FROM track t LEFT JOIN album a ON a.id = t.album_id",
            )?;
            for row in stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })? {
                let (id, text) = row?;
                wanted.insert(("track", id), text);
            }
        }
        let mut changed = 0;
        {
            // Artists, albums and songs; playlists keep rows of their own
            // (playlist_search_row), kept up to date as they change.
            let mut stored: Vec<(i64, String, i64, String)> = Vec::new();
            {
                let mut stmt = tx.prepare(
                    "SELECT rowid, kind, ref_id, text FROM search WHERE kind != 'playlist'",
                )?;
                for row in stmt.query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })? {
                    stored.push(row?);
                }
            }
            let mut delete = tx.prepare_cached("DELETE FROM search WHERE rowid = ?1")?;
            for (rowid, kind, ref_id, text) in &stored {
                let key = match kind.as_str() {
                    "artist" => ("artist", *ref_id),
                    "album" => ("album", *ref_id),
                    _ => ("track", *ref_id),
                };
                if wanted.get(&key) == Some(text) {
                    wanted.remove(&key);
                } else {
                    delete.execute([rowid])?;
                    changed += 1;
                }
            }
            // New rows get IDs below the playlists' (an automatic ID would
            // follow the highest, a playlist's, and could take a future
            // playlist's place).
            let first: i64 = tx.query_row(
                "SELECT COALESCE(MAX(rowid), 0) + 1 FROM search WHERE rowid < ?1",
                [playlist_search_row(0)],
                |row| row.get(0),
            )?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO search (rowid, kind, ref_id, text) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut rows: Vec<_> = wanted.into_iter().collect();
            // Artists, then albums, then songs, each by ID, as before.
            let order = |kind: &str| match kind {
                "artist" => 0,
                "album" => 1,
                _ => 2,
            };
            rows.sort_by_key(|((kind, id), _)| (order(kind), *id));
            for (rowid, ((kind, id), text)) in (first..).zip(rows) {
                insert.execute(params![rowid, kind, id, text])?;
                changed += 1;
            }
            // Playlists without a row (the first build).
            tx.execute(
                "INSERT INTO search (rowid, kind, ref_id, text)
                   SELECT ?1 + p.id, 'playlist', p.id, p.name FROM playlist p
                   WHERE NOT EXISTS (SELECT 1 FROM search s WHERE s.rowid = ?1 + p.id)",
                [playlist_search_row(0)],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }

    fn index_playlist(&mut self, id: i64, name: &str) -> Result<()> {
        let row = playlist_search_row(id);
        self.conn
            .execute("DELETE FROM search WHERE rowid = ?1", [row])?;
        self.conn.execute(
            "INSERT INTO search (rowid, kind, ref_id, text) VALUES (?1, 'playlist', ?2, ?3)",
            params![row, id, name],
        )?;
        Ok(())
    }

    /// Searches everything; every word must match (as a substring).
    /// Words of 3+ characters use the trigram index, shorter ones LIKE.
    pub fn search(&self, query: &str, limit: usize) -> Result<SearchResults> {
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.is_empty() {
            return Ok(SearchResults::default());
        }
        let long: Vec<String> = words
            .iter()
            .filter(|w| w.chars().count() >= 3)
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect();
        let short: Vec<String> = words
            .iter()
            .filter(|w| w.chars().count() < 3)
            .map(|w| {
                format!(
                    "%{}%",
                    w.replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
            })
            .collect();
        let mut sql = String::from("SELECT kind, ref_id FROM search WHERE 1");
        let mut values: Vec<String> = Vec::new();
        if !long.is_empty() {
            sql.push_str(" AND search MATCH ?");
            values.push(long.join(" AND "));
        }
        for pattern in short {
            sql.push_str(" AND text LIKE ? ESCAPE '\\'");
            values.push(pattern);
        }
        sql.push_str(if long.is_empty() {
            " ORDER BY rowid"
        } else {
            " ORDER BY rank"
        });
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut results = SearchResults::default();
        for row in rows {
            let (kind, id) = row?;
            let list = match kind.as_str() {
                "artist" => &mut results.artists,
                "album" => &mut results.albums,
                "track" => &mut results.tracks,
                _ => &mut results.playlists,
            };
            if list.len() < limit {
                list.push(id);
            }
        }
        Ok(results)
    }

    // Queue -----------------------------------------------------------------

    pub fn save_queue(&mut self, snapshot: &QueueSnapshot) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM queue_item", [])?;
        tx.execute(
            "INSERT OR REPLACE INTO queue_state
               (id, current, position_ms, shuffle, repeat, context_album)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![
                snapshot.current.map(|c| c as i64),
                snapshot.position_ms as i64,
                snapshot.shuffle,
                snapshot.repeat.as_str(),
                snapshot.context_album,
            ],
        )?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO queue_item (position, track_id, origin, unshuffled_position)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (position, item) in snapshot.items.iter().enumerate() {
                stmt.execute(params![
                    position as i64,
                    item.track_id,
                    item.origin.as_str(),
                    item.unshuffled_position.map(|p| p as i64),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Each album's genres from its songs' tags, most common first. Case
    /// variants count as one ("K-Pop", "K-POP"); the most used spelling wins.
    /// Each album's barcode (from its songs' tags), to recognize albums
    /// on Spotify (§9.8).
    pub fn album_barcodes(&self) -> Result<HashMap<i64, String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT album_id, max(barcode) FROM track
             WHERE album_id IS NOT NULL AND barcode IS NOT NULL AND barcode != ''
             GROUP BY album_id",
        )?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()
            .map_err(Into::into)
    }

    pub fn album_genres(&self) -> Result<HashMap<i64, Vec<String>>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT album_id, genres FROM track WHERE album_id IS NOT NULL AND genres != ''",
        )?;
        // album → lowercase genre → (count, spelling → count)
        let mut counts: HashMap<i64, HashMap<String, GenreCount>> = HashMap::new();
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (album, genres) = row?;
            for genre in split_list(genres) {
                let genre = genre.trim().to_owned();
                if genre.is_empty() {
                    continue;
                }
                let entry = counts
                    .entry(album)
                    .or_default()
                    .entry(genre.to_lowercase())
                    .or_default();
                entry.0 += 1;
                *entry.1.entry(genre).or_default() += 1;
            }
        }
        Ok(counts
            .into_iter()
            .map(|(album, genres)| {
                let mut genres: Vec<(u32, String)> = genres
                    .into_values()
                    .map(|(count, spellings)| {
                        let spelling = spellings
                            .into_iter()
                            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                            .map(|(s, _)| s)
                            .unwrap_or_default();
                        (count, spelling)
                    })
                    .collect();
                genres.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                (album, genres.into_iter().map(|(_, g)| g).collect())
            })
            .collect())
    }

    // Analysis (§7) -----------------------------------------------------

    /// Songs still to analyze: no content key yet, or no (current) result
    /// for it. Songs without ReplayGain tags first (their loudness sets
    /// their volume), then in library order, `limit` at a time.
    pub fn tracks_to_analyze(&self, limit: usize) -> Result<Vec<(i64, PathBuf)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.id, t.path FROM track t
             LEFT JOIN analysis a ON a.key = t.content_key
             WHERE t.content_key IS NULL OR a.key IS NULL
                OR (a.failed = 0 AND a.features_version < ?1)
             ORDER BY (t.replaygain_track IS NULL AND t.replaygain_album IS NULL) DESC, t.id
             LIMIT ?2",
        )?;
        stmt.query_map(
            params![i64::from(crate::analysis::FEATURES_VERSION), limit as i64],
            |row| Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?))),
        )?
        .collect::<rusqlite::Result<_>>()
        .map_err(Into::into)
    }

    pub fn set_content_key(&mut self, track_id: i64, key: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE track SET content_key = ?2 WHERE id = ?1",
            params![track_id, key],
        )?;
        Ok(())
    }

    /// Whether `key` has a current result (or failed before).
    pub fn has_analysis(&self, key: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM analysis WHERE key = ?1 AND (failed = 1 OR features_version >= ?2)",
                params![key, i64::from(crate::analysis::FEATURES_VERSION)],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Stores a result, or (`None`) that the file couldn't be analyzed.
    pub fn save_analysis(
        &mut self,
        key: &str,
        result: Option<&crate::analysis::SongAnalysis>,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO analysis (key, features, features_version, bpm, failed, analyzed,
               lead_in_ms, outro_ms, end_ms, loudness)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(key) DO UPDATE SET features = excluded.features,
               features_version = excluded.features_version, bpm = excluded.bpm,
               failed = excluded.failed, analyzed = excluded.analyzed,
               lead_in_ms = excluded.lead_in_ms, outro_ms = excluded.outro_ms,
               end_ms = excluded.end_ms, loudness = excluded.loudness",
            params![
                key,
                result.map(|r| crate::analysis::features_to_blob(&r.features)),
                result.map(|r| i64::from(r.features_version)),
                result.and_then(|r| r.bpm),
                result.is_none(),
                now,
                result.and_then(|r| r.edges).map(|e| e.lead_in_ms as i64),
                result.and_then(|r| r.edges).map(|e| e.outro_ms as i64),
                result.and_then(|r| r.edges).map(|e| e.end_ms as i64),
                result.and_then(|r| r.loudness).map(f64::from),
            ],
        )?;
        Ok(())
    }

    /// Where a library song's music starts and ends (smart crossfade);
    /// `None` until it's analyzed.
    pub fn song_edges(&self, track_id: i64) -> Result<Option<crate::analysis::Edges>> {
        Ok(self
            .conn
            .query_row(
                "SELECT a.lead_in_ms, a.outro_ms, a.end_ms FROM track t
                 JOIN analysis a ON a.key = t.content_key
                 WHERE t.id = ?1 AND a.end_ms IS NOT NULL",
                [track_id],
                |row| {
                    Ok(crate::analysis::Edges {
                        lead_in_ms: row.get::<_, i64>(0)?.max(0) as u64,
                        outro_ms: row.get::<_, i64>(1)?.max(0) as u64,
                        end_ms: row.get::<_, i64>(2)?.max(0) as u64,
                    })
                },
            )
            .optional()?)
    }

    /// The gain (dB, ReplayGain's −18 LUFS reference) a song without
    /// ReplayGain tags gets from its measured loudness: its own, or with
    /// `album`, its album's (all of it at one level, so gapless changes
    /// stay seamless). `None` for tagged songs and ones not analyzed.
    pub fn measured_gain(&self, track_id: i64, album: bool) -> Result<Option<f64>> {
        // The analysis listens in mono, BS.1770 adds the channels up: a
        // stereo song measures louder, 3.6 dB going by the ReplayGain tags
        // of real libraries (3 dB for identical channels, a little more
        // for wide stereo).
        let sql = if album {
            "SELECT -18.0 - 10.0 * log10(avg(power(10.0, (a.loudness
                      + CASE WHEN o.channels >= 2 THEN 3.6 ELSE 0.0 END) / 10.0)))
             FROM track t
             JOIN track o ON o.album_id = t.album_id
             JOIN analysis a ON a.key = o.content_key AND a.loudness IS NOT NULL
             WHERE t.id = ?1 AND t.replaygain_track IS NULL AND t.replaygain_album IS NULL
               AND o.replaygain_track IS NULL AND o.replaygain_album IS NULL"
        } else {
            "SELECT -18.0 - (a.loudness + CASE WHEN t.channels >= 2 THEN 3.6 ELSE 0.0 END)
             FROM track t
             JOIN analysis a ON a.key = t.content_key AND a.loudness IS NOT NULL
             WHERE t.id = ?1 AND t.replaygain_track IS NULL AND t.replaygain_album IS NULL"
        };
        Ok(self
            .conn
            .query_row(sql, [track_id], |row| row.get::<_, Option<f64>>(0))
            .optional()?
            .flatten())
    }

    /// (analyzed, all) songs, for the progress in Preferences.
    pub fn analysis_progress(&self) -> Result<(u32, u32)> {
        Ok(self.conn.query_row(
            "SELECT count(a.key), count(*) FROM track t
             LEFT JOIN analysis a ON a.key = t.content_key
               AND (a.failed = 1 OR a.features_version >= ?1)",
            params![i64::from(crate::analysis::FEATURES_VERSION)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    /// Every song's features for similarity (§7).
    pub fn similarity_features(&self) -> Result<Vec<crate::similar::SongFeatures>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.id, t.album_id, coalesce(nullif(t.album_artist, ''), t.artist),
                    t.genres, t.bpm, a.bpm, a.features
             FROM track t
             LEFT JOIN analysis a ON a.key = t.content_key AND a.failed = 0
               AND a.features_version >= ?1",
        )?;
        stmt.query_map(
            params![i64::from(crate::analysis::FEATURES_VERSION)],
            |row| {
                let tag_bpm: Option<f64> = row.get(4)?;
                let detected: Option<f64> = row.get(5)?;
                let sound: Option<Vec<u8>> = row.get(6)?;
                Ok(crate::similar::SongFeatures {
                    track_id: row.get(0)?,
                    album_id: row.get(1)?,
                    artist: row.get::<_, Option<String>>(2)?.map(|a| a.to_lowercase()),
                    genres: split_list(row.get::<_, Option<String>>(3)?.unwrap_or_default())
                        .into_iter()
                        .map(|g| g.trim().to_lowercase())
                        .collect(),
                    bpm: tag_bpm.filter(|b| *b > 0.0).or(detected),
                    sound: sound.map(|b| crate::analysis::features_from_blob(&b)),
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()
        .map_err(Into::into)
    }

    /// Library artists' MusicBrainz IDs whose related artists weren't asked
    /// for since `since`.
    pub fn artists_without_related(&self, since: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT coalesce(a.mbid, a.lookup_mbid) FROM artist a
             WHERE coalesce(a.mbid, a.lookup_mbid) IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM similar_artist_checked c
                               WHERE c.artist_mbid = coalesce(a.mbid, a.lookup_mbid)
                                 AND c.checked >= ?1)
             ORDER BY 1",
        )?;
        let rows = stmt.query_map([since], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// An artist's related artists: (MusicBrainz ID, score).
    pub fn set_related_artists(
        &mut self,
        mbid: &str,
        related: &[(String, i64)],
        now: i64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM similar_artist WHERE artist_mbid = ?1", [mbid])?;
        for (other, score) in related {
            tx.execute(
                "INSERT OR REPLACE INTO similar_artist (artist_mbid, similar_mbid, score)
                 VALUES (?1, ?2, ?3)",
                params![mbid, other, score],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO similar_artist_checked (artist_mbid, checked) VALUES (?1, ?2)",
            params![mbid, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Related artists among the library's, by lowercase name, with how
    /// close they are (score over the artist's best score). Joined here,
    /// not in SQL: no index covers coalesce(mbid, lookup_mbid), and
    /// SQLite's lower() only knows A to Z (`Édith Piaf`).
    pub fn related_artists(&self) -> Result<crate::similar::RelatedArtists> {
        let mut names: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT coalesce(mbid, lookup_mbid), name FROM artist
                 WHERE coalesce(mbid, lookup_mbid) IS NOT NULL",
            )?;
            for row in stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })? {
                let (mbid, name) = row?;
                names.entry(mbid).or_default().push(name.to_lowercase());
            }
        }
        let mut related = crate::similar::RelatedArtists::new();
        let mut best: HashMap<String, i64> = HashMap::new();
        let mut rows = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT artist_mbid, similar_mbid, score FROM similar_artist")?;
            for row in stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })? {
                let (artist, other, score) = row?;
                let top = best.entry(artist.clone()).or_default();
                *top = (*top).max(score);
                rows.push((artist, other, score));
            }
        }
        for (artist, other, score) in rows {
            let (Some(artist_names), Some(other_names)) = (names.get(&artist), names.get(&other))
            else {
                continue;
            };
            let top = best.get(&artist).copied().unwrap_or(0);
            let closeness = if top > 0 {
                score as f64 / top as f64
            } else {
                0.0
            };
            for name in artist_names {
                for other_name in other_names.iter().filter(|o| *o != name) {
                    related
                        .entry(name.clone())
                        .or_default()
                        .insert(other_name.clone(), closeness);
                }
            }
        }
        Ok(related)
    }

    /// Songs played since `since` (unix seconds), to keep them out of mixes.
    pub fn played_since(&self, since: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT DISTINCT track_id FROM play_history WHERE played >= ?1")?;
        stmt.query_map(params![since], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()
            .map_err(Into::into)
    }

    /// Records that a song was played; keeps the newest `HISTORY_KEEP`.
    /// Library songs and streamed Spotify songs (`-stream_track.id`);
    /// other IDs are ignored.
    pub fn add_history(&mut self, track_id: i64, now: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO play_history (track_id, played) SELECT ?1, ?2
             WHERE EXISTS (SELECT 1 FROM track WHERE id = ?1)
                OR EXISTS (SELECT 1 FROM stream_track WHERE id = -?1)",
            params![track_id, now],
        )?;
        tx.execute(
            "DELETE FROM play_history WHERE id <= (SELECT max(id) FROM play_history) - ?1",
            params![HISTORY_KEEP],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Forgets the play history ("Clear" in the queue).
    pub fn clear_history(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM play_history", [])?;
        Ok(())
    }

    /// The playlists that have this song (`track_id`: a library song, or
    /// a streamed one as `-stream_track.id`; `uri`: its Spotify URI, when
    /// known otherwise): as itself, or as the same song on Spotify.
    pub fn playlists_containing(&self, track_id: i64, uri: Option<&str>) -> Result<Vec<i64>> {
        let mut playlists: Vec<i64> = self
            .song_entries(track_id, uri)?
            .into_iter()
            .map(|(playlist, _)| playlist)
            .collect();
        playlists.dedup();
        Ok(playlists)
    }

    /// The playlist entries of this song (see `playlists_containing`), as
    /// (playlist, entry) pairs ordered by playlist.
    pub fn song_entries(&self, track_id: i64, uri: Option<&str>) -> Result<Vec<(i64, i64)>> {
        let mut stmt = self.conn.prepare_cached(
            "WITH uri(uri) AS (
                 SELECT uri FROM stream_track WHERE id = -?1
                 UNION SELECT uri FROM spotify_track
                   WHERE track_id = ?1 AND confidence = 'sure' AND uri IS NOT NULL
                 UNION SELECT ?2 WHERE ?2 IS NOT NULL
             ), local(id) AS (
                 SELECT ?1 WHERE ?1 > 0
                 UNION SELECT track_id FROM spotify_track
                   WHERE confidence = 'sure' AND uri IN (SELECT uri FROM uri)
             )
             SELECT playlist_id, id FROM playlist_item
             WHERE spotify_uri IN (SELECT uri FROM uri)
                OR local_track_id IN (SELECT id FROM local)
             ORDER BY playlist_id, position",
        )?;
        stmt.query_map(params![track_id, uri], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()
            .map_err(Into::into)
    }

    /// The last `limit` played songs, oldest first.
    pub fn history(&self, limit: usize) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT track_id FROM play_history ORDER BY id DESC LIMIT ?1")?;
        let mut ids = stmt
            .query_map(params![limit as i64], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        ids.reverse();
        Ok(ids)
    }

    pub fn load_queue(&self) -> Result<Option<QueueSnapshot>> {
        let state = self
            .conn
            .query_row(
                "SELECT current, position_ms, shuffle, repeat, context_album FROM queue_state",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((current, position_ms, shuffle, repeat, context_album)) = state else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT track_id, origin, unshuffled_position FROM queue_item ORDER BY position",
        )?;
        let items = stmt
            .query_map([], |row| {
                Ok(crate::queue::SnapshotItem {
                    track_id: row.get(0)?,
                    origin: Origin::from_str_lossy(&row.get::<_, String>(1)?),
                    unshuffled_position: row.get::<_, Option<i64>>(2)?.map(|p| p as usize),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(QueueSnapshot {
            items,
            current: current.map(|c| c as usize),
            position_ms: position_ms.max(0) as u64,
            shuffle,
            repeat: RepeatMode::from_str_lossy(&repeat),
            context_album,
        }))
    }
}

const TRACK_COLUMNS: &str = "t.id, t.path, t.added, t.title, t.artist, t.album, t.album_artist,
    t.album_artists, t.album_artist_sort, t.disc, t.number, t.year, t.genres, t.composer,
    t.mb_album_id, t.mb_release_group_id, t.mb_track_id, t.mb_album_artist_ids,
    t.release_comment, t.release_type, t.compilation, t.isrc, t.bpm, t.lyrics_synced,
    t.lyrics_excerpt, t.replaygain_track, t.replaygain_album, t.duration_ms, t.format,
    t.bitrate, t.sample_rate, t.bit_depth, t.channels, t.cover_key, t.barcode";
const TRACK_COLUMN_COUNT: usize = 35;
const TRACK_ALBUM_COLUMNS: &str = "t.album_id, a.title, a.artist_credit, a.cover_key, t.album_disc";

const ALBUM_COLUMNS: &str = "id, group_key, title, sort_title, edition, artist_credit,
    artist_sort, year, release_type, soundtrack, compilation, track_count, duration_ms,
    cover_key, added";

/// How often a genre occurs, and how often in each spelling.
type GenreCount = (u32, HashMap<String, u32>);

/// Play history entries kept in the database.
const HISTORY_KEEP: i64 = 200;

/// A playlist's row in the search index: derived from its ID, after the
/// rows of artists, albums and songs (numbered from 1 by each rebuild).
fn playlist_search_row(id: i64) -> i64 {
    (1 << 40) + id
}

const UPSERT_TRACK: &str = "INSERT INTO track (path, mtime, size, added, title, artist, album,
    album_artist, album_artists, album_artist_sort, disc, number, year, genres, composer,
    mb_album_id, mb_release_group_id, mb_track_id, mb_album_artist_ids, release_comment,
    release_type, compilation, isrc, bpm, lyrics_synced, lyrics_excerpt, replaygain_track,
    replaygain_album, duration_ms, format, bitrate, sample_rate, bit_depth, channels, cover_key,
    barcode)
  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
    ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36)
  ON CONFLICT(path) DO UPDATE SET mtime = excluded.mtime, size = excluded.size,
    content_key = NULL,
    title = excluded.title, artist = excluded.artist, album = excluded.album,
    album_artist = excluded.album_artist, album_artists = excluded.album_artists,
    album_artist_sort = excluded.album_artist_sort, disc = excluded.disc,
    number = excluded.number, year = excluded.year, genres = excluded.genres,
    composer = excluded.composer, mb_album_id = excluded.mb_album_id,
    mb_release_group_id = excluded.mb_release_group_id, mb_track_id = excluded.mb_track_id,
    mb_album_artist_ids = excluded.mb_album_artist_ids,
    release_comment = excluded.release_comment, release_type = excluded.release_type,
    compilation = excluded.compilation, isrc = excluded.isrc, bpm = excluded.bpm,
    lyrics_synced = excluded.lyrics_synced, lyrics_excerpt = excluded.lyrics_excerpt,
    replaygain_track = excluded.replaygain_track, replaygain_album = excluded.replaygain_album,
    duration_ms = excluded.duration_ms, format = excluded.format, bitrate = excluded.bitrate,
    sample_rate = excluded.sample_rate, bit_depth = excluded.bit_depth,
    channels = excluded.channels, cover_key = excluded.cover_key, barcode = excluded.barcode";

const UPSERT_ALBUM: &str = "INSERT INTO album (key, group_key, title, sort_title, edition,
    artist_credit, artist_sort, year, release_type, soundtrack, compilation, mb_album_id,
    mb_release_group_id, track_count, duration_ms, cover_key, added)
  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
  ON CONFLICT(key) DO UPDATE SET group_key = excluded.group_key, title = excluded.title,
    sort_title = excluded.sort_title, edition = excluded.edition,
    artist_credit = excluded.artist_credit, artist_sort = excluded.artist_sort,
    year = excluded.year, release_type = excluded.release_type,
    soundtrack = excluded.soundtrack, compilation = excluded.compilation,
    mb_album_id = excluded.mb_album_id, mb_release_group_id = excluded.mb_release_group_id,
    track_count = excluded.track_count, duration_ms = excluded.duration_ms,
    cover_key = excluded.cover_key, added = excluded.added
  RETURNING id";

const PLAYLIST_COLUMNS: &str = "p.id, p.name, p.custom_cover_key, p.created, p.modified,
    (SELECT COUNT(*) FROM playlist_item i WHERE i.playlist_id = p.id),
    (SELECT COUNT(*) FROM playlist_item i WHERE i.playlist_id = p.id AND i.local_track_id IS NOT NULL),
    (SELECT COALESCE(SUM(COALESCE(t.duration_ms, i.duration_ms, 0)), 0) FROM playlist_item i
       LEFT JOIN track t ON t.id = i.local_track_id WHERE i.playlist_id = p.id),
    p.source, p.spotify_id, p.spotify_owner, p.editable, p.readable, p.spotify_cover_key,
    (SELECT COUNT(*) FROM spotify_outbox o WHERE o.playlist_id = p.id)";

const INSERT_ITEM: &str = "INSERT INTO playlist_item (playlist_id, position, local_track_id,
    isrc, title, artist, album, duration_ms, path, mb_track_id, match_state)
  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

fn playlist(row: &Row) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: row.get(0)?,
        name: row.get(1)?,
        custom_cover_key: row.get(2)?,
        created: row.get(3)?,
        modified: row.get(4)?,
        item_count: row.get(5)?,
        local_count: row.get(6)?,
        duration_ms: row.get::<_, i64>(7)?.max(0) as u64,
        cover_keys: Vec::new(),
        source: row.get(8)?,
        spotify_id: row.get(9)?,
        spotify_owner: row.get(10)?,
        editable: row.get(11)?,
        readable: row.get(12)?,
        spotify_cover_key: row.get(13)?,
        pending_changes: row.get(14)?,
    })
}

/// What an entry knows about its track, for (re-)matching.
struct Snapshot {
    path: Option<String>,
    mb_track_id: Option<String>,
    isrc: Option<String>,
    title: Option<String>,
    artist: Option<String>,
    duration_ms: Option<u64>,
}

/// The library track an entry refers to: by path, MusicBrainz track ID,
/// ISRC, or title + artist (case-insensitive) with the duration ±3 s.
fn find_track(conn: &Connection, s: &Snapshot) -> rusqlite::Result<Option<i64>> {
    let by = |sql: &str, value: &Option<String>| -> rusqlite::Result<Option<i64>> {
        match value {
            Some(value) => conn.query_row(sql, [value], |row| row.get(0)).optional(),
            None => Ok(None),
        }
    };
    if let Some(id) = by("SELECT id FROM track WHERE path = ?1", &s.path)? {
        return Ok(Some(id));
    }
    if let Some(id) = by(
        "SELECT id FROM track WHERE mb_track_id = ?1",
        &s.mb_track_id,
    )? {
        return Ok(Some(id));
    }
    if let Some(id) = by("SELECT id FROM track WHERE isrc = ?1", &s.isrc)? {
        return Ok(Some(id));
    }
    let Some(title) = &s.title else {
        return Ok(None);
    };
    conn.query_row(
        "SELECT id FROM track WHERE lower(title) = lower(?1)
           AND (?2 IS NULL OR lower(artist) = lower(?2) OR lower(album_artist) = lower(?2))
           AND (?3 IS NULL OR abs(duration_ms - ?3) <= 3000)
         ORDER BY CASE format WHEN 'FLAC' THEN 0 WHEN 'MPEG-4' THEN 1 ELSE 2 END, id LIMIT 1",
        params![title, s.artist, s.duration_ms.map(|d| d as i64)],
        |row| row.get(0),
    )
    .optional()
}

/// What's looked up about an artist or chosen for them, kept when the
/// artist's key changes (different aliases from lyrembed, homonyms told
/// apart): the new row of the same name takes the old row's data.
const ARTIST_DATA: &str = "lookup_state, looked_up, lookup_mbid, wikidata_id, spotify_id,
    wikipedia_url, website, bio, portrait_key, portrait_source, portrait_attribution,
    custom_portrait_key";

fn carry_artist_data(tx: &rusqlite::Transaction, kept: &HashSet<i64>) -> Result<()> {
    let rows: Vec<(i64, String, bool)> = {
        let mut stmt = tx.prepare(
            "SELECT id, lower(name), lookup_state IS NULL AND custom_portrait_key IS NULL
             FROM artist",
        )?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let leaving: HashMap<&str, i64> = rows
        .iter()
        .filter(|(id, _, _)| !kept.contains(id))
        .map(|(id, name, _)| (name.as_str(), *id))
        .collect();
    let mut copy = tx.prepare(&format!(
        "UPDATE artist SET ({ARTIST_DATA}) = (SELECT {ARTIST_DATA} FROM artist WHERE id = ?2)
         WHERE id = ?1"
    ))?;
    for (id, name, fresh) in &rows {
        if *fresh
            && kept.contains(id)
            && let Some(old) = leaving.get(name.as_str())
        {
            copy.execute(params![id, old])?;
        }
    }
    Ok(())
}

/// An artist row; when another artist of the same name (another
/// MusicBrainz ID) takes over the key, what was looked up about the first
/// one goes (it isn't about this one).
const UPSERT_ARTIST: &str =
    "INSERT INTO artist (key, name, sort_name, mbid) VALUES (?1, ?2, ?3, ?4)
  ON CONFLICT(key) DO UPDATE SET name = excluded.name, sort_name = excluded.sort_name,
    lookup_state = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE lookup_state END,
    looked_up = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE looked_up END,
    lookup_mbid = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE lookup_mbid END,
    wikidata_id = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE wikidata_id END,
    spotify_id = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE spotify_id END,
    wikipedia_url = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE wikipedia_url END,
    website = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE website END,
    bio = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE bio END,
    portrait_key = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE portrait_key END,
    portrait_source = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE portrait_source END,
    portrait_attribution = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE portrait_attribution END,
    custom_portrait_key = CASE WHEN excluded.mbid IS NOT NULL AND COALESCE(artist.mbid, artist.lookup_mbid) IS NOT NULL AND excluded.mbid != COALESCE(artist.mbid, artist.lookup_mbid) THEN NULL ELSE custom_portrait_key END,
    mbid = excluded.mbid
  RETURNING id";

const ARTIST_COLUMNS: &str =
    "ar.id, ar.key, ar.name, ar.sort_name, COALESCE(ar.mbid, ar.lookup_mbid),
    (SELECT COUNT(*) FROM album_artist aa WHERE aa.artist_id = ar.id AND aa.role = 'credit'),
    (SELECT COALESCE(SUM(a.track_count), 0) FROM album_artist aa JOIN album a ON a.id = aa.album_id
       WHERE aa.artist_id = ar.id AND aa.role = 'credit'),
    COALESCE(ar.custom_portrait_key, ar.portrait_key),
    (SELECT a.cover_key FROM album_artist aa JOIN album a ON a.id = aa.album_id
       WHERE aa.artist_id = ar.id AND a.cover_key IS NOT NULL
       ORDER BY aa.role = 'credit' DESC, a.year DESC LIMIT 1),
    ar.lookup_state, ar.looked_up, ar.lookup_mbid, ar.wikidata_id, ar.spotify_id,
    ar.wikipedia_url, ar.website, ar.bio, ar.portrait_key, ar.portrait_source,
    ar.portrait_attribution, ar.custom_portrait_key,
    (SELECT COALESCE(MIN(a.added), 0) FROM album_artist aa JOIN album a ON a.id = aa.album_id
       WHERE aa.artist_id = ar.id AND aa.role = 'credit')";

fn artist(row: &Row) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: row.get(0)?,
        key: row.get(1)?,
        name: row.get(2)?,
        sort_name: row.get(3)?,
        mbid: row.get(4)?,
        album_count: row.get(5)?,
        track_count: row.get(6)?,
        portrait_key: row.get(7)?,
        cover_key: row.get(8)?,
        added: row.get(21)?,
        meta: ArtistMeta {
            lookup_state: row.get(9)?,
            looked_up: row.get(10)?,
            lookup_mbid: row.get(11)?,
            wikidata_id: row.get(12)?,
            spotify_id: row.get(13)?,
            wikipedia_url: row.get(14)?,
            website: row.get(15)?,
            bio: row.get(16)?,
            portrait_key: row.get(17)?,
            portrait_source: row.get(18)?,
            portrait_attribution: row.get(19)?,
            custom_portrait_key: row.get(20)?,
        },
    })
}

/// Deletes rows of `table` whose ID isn't in `keep`.
/// A song's album, its place in it and its disc, as stored.
type AlbumLink = (Option<i64>, Option<i64>, Option<i64>);
/// An artist row as `replace_library` compares it: ID, name, sort name,
/// MusicBrainz ID.
type StoredArtist = (i64, Option<String>, Option<String>, Option<String>);

/// The album columns `replace_library` compares, in UPSERT_ALBUM's order
/// after the key.
const ALBUM_STORED: &str = "group_key, title, sort_title, edition, artist_credit, artist_sort,
    year, release_type, soundtrack, compilation, mb_album_id, mb_release_group_id, track_count,
    duration_ms, cover_key, added";
const ALBUM_STORED_COUNT: usize = 16;

/// An album's values as they're stored (for comparing, and UPSERT_ALBUM).
fn album_values(album: &AlbumInfo) -> Result<Vec<Value>> {
    let values: [&dyn rusqlite::ToSql; ALBUM_STORED_COUNT] = [
        &album.group_key,
        &album.title,
        &album.sort_title,
        &album.edition,
        &album.artist_credit,
        &album.artist_sort,
        &album.year,
        &album.release_type.as_str(),
        &album.soundtrack,
        &album.compilation,
        &album.mb_album_id,
        &album.mb_release_group_id,
        &(album.track_ids.len() as i64),
        &(album.duration_ms as i64),
        &album.cover_key,
        &album.added,
    ];
    values
        .iter()
        .map(|value| {
            Ok(match value.to_sql()? {
                rusqlite::types::ToSqlOutput::Borrowed(borrowed) => borrowed.into(),
                rusqlite::types::ToSqlOutput::Owned(owned) => owned,
                _ => Value::Null,
            })
        })
        .collect()
}

/// Brings a link table to `wanted` (links by owner: an album's artists, a
/// song's artists), rewriting only the owners whose links differ.
fn sync_links<T: PartialEq + Ord + Clone>(
    tx: &rusqlite::Transaction,
    select: &str,
    read: impl FnMut(&Row) -> rusqlite::Result<(i64, T)>,
    mut wanted: HashMap<i64, Vec<T>>,
    delete: &str,
    insert: impl Fn(&rusqlite::Transaction, i64, &T) -> rusqlite::Result<()>,
) -> Result<()> {
    let mut stored: HashMap<i64, Vec<T>> = HashMap::new();
    {
        let mut stmt = tx.prepare(select)?;
        for row in stmt.query_map([], read)? {
            let (owner, link) = row?;
            stored.entry(owner).or_default().push(link);
        }
    }
    for links in stored.values_mut().chain(wanted.values_mut()) {
        links.sort();
        links.dedup();
    }
    let mut remove = tx.prepare_cached(delete)?;
    for owner in stored.keys() {
        if !wanted.contains_key(owner) {
            remove.execute([owner])?;
        }
    }
    for (owner, links) in &wanted {
        if stored.get(owner) == Some(links) {
            continue;
        }
        remove.execute([owner])?;
        for link in links {
            insert(tx, *owner, link)?;
        }
    }
    Ok(())
}

fn delete_except(tx: &rusqlite::Transaction, table: &str, keep: &HashSet<i64>) -> Result<()> {
    let stale: Vec<i64> = {
        let mut stmt = tx.prepare(&format!("SELECT id FROM {table}"))?;
        stmt.query_map([], |row| row.get(0))?
            .filter_map(std::result::Result::ok)
            .filter(|id| !keep.contains(id))
            .collect()
    };
    let mut delete = tx.prepare(&format!("DELETE FROM {table} WHERE id = ?1"))?;
    for id in stale {
        delete.execute([id])?;
    }
    Ok(())
}

fn track_record(row: &Row) -> rusqlite::Result<TrackRecord> {
    let lyrics_synced: Option<bool> = row.get(23)?;
    let lyrics_excerpt: Option<String> = row.get(24)?;
    Ok(TrackRecord {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        added: row.get(2)?,
        tags: TrackTags {
            title: row.get(3)?,
            artist: row.get(4)?,
            album: row.get(5)?,
            album_artist: row.get(6)?,
            album_artists: split_list(row.get(7)?),
            album_artist_sort: row.get(8)?,
            disc: row.get(9)?,
            number: row.get(10)?,
            year: row.get(11)?,
            genres: split_list(row.get(12)?),
            composer: row.get(13)?,
            mb_album_id: row.get(14)?,
            mb_release_group_id: row.get(15)?,
            mb_track_id: row.get(16)?,
            mb_album_artist_ids: split_list(row.get(17)?),
            release_comment: row.get(18)?,
            release_type: row.get(19)?,
            compilation: row.get(20)?,
            isrc: row.get(21)?,
            barcode: row.get(34)?,
            bpm: row.get(22)?,
            lyrics: lyrics_synced
                .zip(lyrics_excerpt)
                .map(|(synced, excerpt)| Lyrics { synced, excerpt }),
            replaygain_track: row.get(25)?,
            replaygain_album: row.get(26)?,
            duration_ms: row.get::<_, i64>(27)?.max(0) as u64,
            format: row.get(28)?,
            bitrate: row.get(29)?,
            sample_rate: row.get(30)?,
            bit_depth: row.get(31)?,
            channels: row.get(32)?,
        },
        cover_key: row.get(33)?,
    })
}

fn track(row: &Row) -> rusqlite::Result<Track> {
    let n = TRACK_COLUMN_COUNT;
    Ok(Track {
        record: track_record(row)?,
        album_id: row.get(n)?,
        album_title: row.get(n + 1)?,
        album_artist: row.get(n + 2)?,
        album_cover_key: row.get(n + 3)?,
        album_disc: row.get(n + 4)?,
    })
}

fn album(row: &Row) -> rusqlite::Result<Album> {
    Ok(Album {
        id: row.get(0)?,
        group_key: row.get(1)?,
        title: row.get(2)?,
        sort_title: row.get(3)?,
        edition: row.get(4)?,
        artist_credit: row.get(5)?,
        artist_sort: row.get(6)?,
        year: row.get(7)?,
        release_type: ReleaseType::from_str_lossy(&row.get::<_, String>(8)?),
        soundtrack: row.get(9)?,
        compilation: row.get(10)?,
        track_count: row.get(11)?,
        duration_ms: row.get::<_, i64>(12)?.max(0) as u64,
        cover_key: row.get(13)?,
        added: row.get(14)?,
    })
}

fn path_str(path: &Path) -> &str {
    // Library folders are UTF-8 (folders.rs) and the scanner skips other names.
    path.to_str().unwrap_or_default()
}

fn join_list(values: &[String]) -> String {
    let mut joined = String::new();
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            joined.push(LIST_SEPARATOR);
        }
        joined.push_str(value);
    }
    joined
}

fn split_list(joined: String) -> Vec<String> {
    if joined.is_empty() {
        Vec::new()
    } else {
        joined.split(LIST_SEPARATOR).map(str::to_owned).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::grouping::group_albums;
    use crate::queue::SnapshotItem;

    pub(super) fn file(path: &str, album: &str, title: &str, number: u32) -> ScannedFile {
        ScannedFile {
            path: PathBuf::from(path),
            mtime: 1,
            size: 2,
            tags: TrackTags {
                title: Some(title.into()),
                album: Some(album.into()),
                album_artist: Some("Adele".into()),
                number: Some(number),
                genres: vec!["Pop".into(), "Soul".into()],
                duration_ms: 180_000,
                format: "FLAC".into(),
                lyrics: Some(Lyrics {
                    synced: true,
                    excerpt: "Hello".into(),
                }),
                ..TrackTags::default()
            },
            cover_key: Some("eabc".into()),
        }
    }

    #[test]
    fn romanizations_are_kept_and_misses_asked_again_later() {
        let mut db = Database::open_in_memory().unwrap();
        db.set_romanized("found", Some("Tōkyō"), 100).unwrap();
        db.set_romanized("miss", None, 100).unwrap();
        // Long after: a romanization still counts, "nothing to do" doesn't.
        assert_eq!(
            db.romanized("found", 1_000).unwrap(),
            Some(Some("Tōkyō".to_owned()))
        );
        assert_eq!(db.romanized("miss", 1_000).unwrap(), None);
        assert_eq!(db.romanized("miss", 50).unwrap(), Some(None));
        assert_eq!(db.romanized("other", 0).unwrap(), None);
    }

    #[test]
    fn a_removed_albums_id_isnt_reused() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(&[file("/m/A/1/01.flac", "1", "One", 1)], 1)
            .unwrap();
        rebuild(&mut db);
        let first = db.albums().unwrap()[0].id;
        db.delete_tracks(&[PathBuf::from("/m/A/1/01.flac")])
            .unwrap();
        rebuild(&mut db);
        assert!(db.albums().unwrap().is_empty());
        db.upsert_tracks(&[file("/m/A/2/01.flac", "2", "Two", 1)], 1)
            .unwrap();
        rebuild(&mut db);
        let albums = db.albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_ne!(albums[0].id, first);
    }

    #[test]
    fn another_artist_of_the_same_name_doesnt_inherit_the_data() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(&[file("/m/A/1/01.flac", "1", "One", 1)], 1)
            .unwrap();
        let tracks = db.all_tracks().unwrap();
        let albums = group_albums(&tracks);
        let mut artists =
            crate::library::artists::build_artists(&albums, &tracks, &Default::default());
        artists[0].mbid = Some("us-nirvana".into());
        db.replace_library(&albums, &artists).unwrap();
        let first = db.artists().unwrap()[0].clone();
        let meta = ArtistMeta {
            lookup_state: Some("done".into()),
            bio: Some("From Seattle".into()),
            ..ArtistMeta::default()
        };
        db.set_artist_meta(first.id, &meta).unwrap();
        // Same key, the same MBID: the data stays.
        db.replace_library(&albums, &artists).unwrap();
        assert_eq!(
            db.artists().unwrap()[0].meta.bio.as_deref(),
            Some("From Seattle")
        );
        // Now another artist of that name owns the key.
        artists[0].mbid = Some("uk-nirvana".into());
        db.replace_library(&albums, &artists).unwrap();
        let after = db.artists().unwrap()[0].clone();
        assert_eq!(after.id, first.id);
        assert_eq!(after.meta.bio, None);
        assert_eq!(after.meta.lookup_state, None);
    }

    #[test]
    fn artist_data_survives_a_key_change() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(&[file("/m/A/1/01.flac", "1", "One", 1)], 1)
            .unwrap();
        rebuild(&mut db);
        let adele = db.artists().unwrap()[0].clone();
        let meta = ArtistMeta {
            lookup_state: Some("done".into()),
            bio: Some("Singer".into()),
            ..ArtistMeta::default()
        };
        db.set_artist_meta(adele.id, &meta).unwrap();
        db.set_custom_portrait(adele.id, Some("p1")).unwrap();
        // Other aliases from lyrembed: the same artist gets another key.
        let tracks = db.all_tracks().unwrap();
        let albums = group_albums(&tracks);
        let mut artists =
            crate::library::artists::build_artists(&albums, &tracks, &Default::default());
        artists[0].key = "name:adele adkins".into();
        db.replace_library(&albums, &artists).unwrap();
        let after = db.artists().unwrap();
        assert_eq!(after.len(), 1);
        assert_ne!(after[0].id, adele.id);
        assert_eq!(after[0].meta.bio.as_deref(), Some("Singer"));
        assert_eq!(after[0].meta.custom_portrait_key.as_deref(), Some("p1"));
    }

    #[test]
    fn an_artist_is_added_with_their_first_album() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(&[file("/m/Adele/19/01.flac", "19", "Daydreamer", 1)], 100)
            .unwrap();
        db.upsert_tracks(&[file("/m/Adele/25/01.flac", "25", "Hello", 1)], 300)
            .unwrap();
        let mut other = file("/m/Sia/1000/01.flac", "1000 Forms of Fear", "Chandelier", 1);
        other.tags.album_artist = Some("Sia".into());
        db.upsert_tracks(&[other], 200).unwrap();
        rebuild(&mut db);
        let added: Vec<_> = db
            .artists()
            .unwrap()
            .into_iter()
            .map(|a| (a.name, a.added))
            .collect();
        // Adele came first, with "19"; "25" later doesn't change that.
        assert_eq!(added, [("Adele".into(), 100), ("Sia".into(), 200)]);
    }

    pub(super) fn rebuild(db: &mut Database) {
        let tracks = db.all_tracks().unwrap();
        let albums = group_albums(&tracks);
        let artists = crate::library::artists::build_artists(&albums, &tracks, &Default::default());
        db.replace_library(&albums, &artists).unwrap();
    }

    #[test]
    fn tracks_round_trip_and_keep_ids() {
        let mut db = Database::open_in_memory().unwrap();
        let mut f = file("/m/Adele/25/01.flac", "25", "Hello", 1);
        db.upsert_tracks(std::slice::from_ref(&f), 100).unwrap();
        let first = db.all_tracks().unwrap();
        assert_eq!(first[0].tags, f.tags);

        f.tags.title = Some("Hello (Live)".into());
        db.upsert_tracks(std::slice::from_ref(&f), 200).unwrap();
        let second = db.all_tracks().unwrap();
        assert_eq!(second[0].id, first[0].id);
        assert_eq!(second[0].added, 100);
        assert_eq!(second[0].tags.title.as_deref(), Some("Hello (Live)"));
    }

    #[test]
    fn albums_are_rebuilt_with_stable_ids() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(
            &[
                file("/m/Adele/25/02.flac", "25", "Send My Love", 2),
                file("/m/Adele/25/01.flac", "25", "Hello", 1),
                file("/m/Adele/21/01.flac", "21", "Rolling in the Deep", 1),
            ],
            1,
        )
        .unwrap();
        rebuild(&mut db);
        let albums = db.albums().unwrap();
        assert_eq!(albums.len(), 2);
        let album_25 = albums.iter().find(|a| a.title == "25").unwrap().clone();
        let tracks = db.album_tracks(album_25.id).unwrap();
        let titles: Vec<_> = tracks
            .iter()
            .map(|t| t.record.tags.title.clone().unwrap())
            .collect();
        assert_eq!(titles, ["Hello", "Send My Love"]);
        assert_eq!(tracks[0].album_title.as_deref(), Some("25"));

        db.delete_tracks(&[PathBuf::from("/m/Adele/21/01.flac")])
            .unwrap();
        rebuild(&mut db);
        let albums = db.albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].id, album_25.id);
        assert_eq!(db.album_cover_sources().unwrap().len(), 1);
    }

    fn track_ids(db: &Database) -> HashMap<String, i64> {
        db.all_tracks()
            .unwrap()
            .into_iter()
            .map(|t| (t.tags.title.clone().unwrap(), t.id))
            .collect()
    }

    #[test]
    fn a_playlist_keeps_its_sort_and_when_songs_were_added() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(&[file("/m/Adele/25/01.flac", "25", "Hello", 1)], 1)
            .unwrap();
        rebuild(&mut db);
        let ids = track_ids(&db);
        let playlist = db.create_playlist("Mix", 5).unwrap();
        assert_eq!(db.playlist_sort(playlist).unwrap(), None);
        db.set_playlist_sort(playlist, Some(("added", true)))
            .unwrap();
        assert_eq!(
            db.playlist_sort(playlist).unwrap(),
            Some(("added".to_owned(), true))
        );
        // The own order, chosen, isn't the default.
        db.set_playlist_sort(playlist, Some(("custom", false)))
            .unwrap();
        assert_eq!(
            db.playlist_sort(playlist).unwrap(),
            Some(("custom".to_owned(), false))
        );
        db.set_playlist_sort(playlist, None).unwrap();
        assert_eq!(db.playlist_sort(playlist).unwrap(), None);
        // Only the library's songs: also remembered, apart from the sort.
        assert!(!db.playlist_library_only(playlist).unwrap());
        db.set_playlist_library_only(playlist, true).unwrap();
        db.set_playlist_sort(playlist, Some(("title", false)))
            .unwrap();
        assert!(db.playlist_library_only(playlist).unwrap());
        assert!(!db.playlist_library_only(playlist + 1).unwrap());
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        db.add_to_playlist(playlist, &[ids["Hello"]], None, 6)
            .unwrap();
        let added = db.playlist_items(playlist).unwrap()[0].added.unwrap();
        assert!((before - 1..=before + 5).contains(&added), "{added}");
    }

    #[test]
    fn songs_that_are_gone_leave_no_gap_in_a_playlist() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(
            &[
                file("/m/Adele/25/01.flac", "25", "Hello", 1),
                file("/m/Adele/25/02.flac", "25", "Send My Love", 2),
            ],
            1,
        )
        .unwrap();
        rebuild(&mut db);
        let ids = track_ids(&db);
        let playlist = db.create_playlist("Mix", 5).unwrap();
        db.add_to_playlist(playlist, &[ids["Hello"]], None, 6)
            .unwrap();
        // 999 was removed by a scan before it got here.
        db.add_to_playlist(playlist, &[999, ids["Send My Love"]], Some(0), 7)
            .unwrap();
        let positions: Vec<i64> = {
            let mut stmt = db
                .conn
                .prepare("SELECT position FROM playlist_item ORDER BY position")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(positions, [0, 1]);
    }

    #[test]
    fn playlists_add_move_remove_and_relink() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(
            &[
                file("/m/Adele/25/01.flac", "25", "Hello", 1),
                file("/m/Adele/25/02.flac", "25", "Send My Love", 2),
                file("/m/Adele/21/01.flac", "21", "Rolling in the Deep", 1),
            ],
            1,
        )
        .unwrap();
        rebuild(&mut db);
        let ids = track_ids(&db);
        let playlist = db.create_playlist("Mix", 5).unwrap();
        db.add_to_playlist(
            playlist,
            &[ids["Hello"], ids["Rolling in the Deep"]],
            None,
            6,
        )
        .unwrap();
        db.add_to_playlist(playlist, &[ids["Send My Love"]], Some(0), 7)
            .unwrap();
        let titles = |db: &Database| -> Vec<String> {
            db.playlist_items(playlist)
                .unwrap()
                .iter()
                .map(|i| i.title.clone().unwrap())
                .collect()
        };
        assert_eq!(
            titles(&db),
            ["Send My Love", "Hello", "Rolling in the Deep"]
        );

        db.move_playlist_item(playlist, 2, 0, 8).unwrap();
        assert_eq!(
            titles(&db),
            ["Rolling in the Deep", "Send My Love", "Hello"]
        );
        let info = db.playlist(playlist).unwrap().unwrap();
        assert_eq!((info.item_count, info.local_count), (3, 3));
        assert_eq!(info.cover_keys, ["eabc"]);

        // The file moves: the entry finds it again by title and duration.
        db.delete_tracks(&[PathBuf::from("/m/Adele/25/01.flac")])
            .unwrap();
        db.relink_playlists().unwrap();
        assert_eq!(
            db.playlist_items(playlist).unwrap()[2].match_state,
            "unavailable"
        );
        db.upsert_tracks(&[file("/m/Adele/25 (2015)/01.flac", "25", "Hello", 1)], 9)
            .unwrap();
        rebuild(&mut db);
        db.relink_playlists().unwrap();
        let items = db.playlist_items(playlist).unwrap();
        assert_eq!(items[2].match_state, "local");
        assert_eq!(
            items[2].track.as_ref().unwrap().record.path,
            PathBuf::from("/m/Adele/25 (2015)/01.flac")
        );

        let first = items[0].id;
        db.remove_playlist_item(playlist, first, 10).unwrap();
        assert_eq!(titles(&db), ["Send My Love", "Hello"]);
        db.delete_playlist(playlist).unwrap();
        assert!(db.playlists().unwrap().is_empty());
    }

    #[test]
    fn imports_match_by_path_and_title() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(&[file("/m/Adele/25/01.flac", "25", "Hello", 1)], 1)
            .unwrap();
        rebuild(&mut db);
        let playlist = db.create_playlist("Imported", 1).unwrap();
        let entries = vec![
            PlaylistFileEntry {
                location: Some(PathBuf::from("/m/Adele/25/01.flac")),
                ..Default::default()
            },
            PlaylistFileEntry {
                title: Some("hello".into()),
                artist: Some("ADELE".into()),
                duration_ms: Some(181_000),
                ..Default::default()
            },
            PlaylistFileEntry {
                title: Some("Unknown".into()),
                ..Default::default()
            },
        ];
        assert_eq!(db.import_entries(playlist, &entries, 2).unwrap(), 2);
        let states: Vec<String> = db
            .playlist_items(playlist)
            .unwrap()
            .into_iter()
            .map(|i| i.match_state)
            .collect();
        assert_eq!(states, ["local", "local", "unavailable"]);
    }

    #[test]
    fn an_unchanged_library_is_not_written_again() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(
            &[
                file("/m/Adele/25/01.flac", "25", "Hello", 1),
                file("/m/Adele/25/02.flac", "25", "Send My Love", 2),
                file("/m/Adele/21/01.flac", "21", "Rolling", 1),
            ],
            1,
        )
        .unwrap();
        rebuild(&mut db);
        let aliases = Aliases::default();
        assert!(db.rebuild_search(&aliases).unwrap() > 0);
        let albums_before = db.albums().unwrap();
        let regroup = |db: &mut Database| {
            let tracks = db.all_tracks().unwrap();
            let albums = group_albums(&tracks);
            let artists =
                crate::library::artists::build_artists(&albums, &tracks, &Default::default());
            db.replace_library(&albums, &artists).unwrap()
        };
        // Nothing changed: nothing written, same IDs.
        assert_eq!(regroup(&mut db), (0, 0, 0));
        assert_eq!(db.rebuild_search(&aliases).unwrap(), 0);
        assert_eq!(db.albums().unwrap(), albums_before);
        // One song renamed: its album (track list unchanged) stays, its
        // search row is replaced.
        db.upsert_tracks(&[file("/m/Adele/25/01.flac", "25", "Hello Again", 1)], 2)
            .unwrap();
        assert_eq!(regroup(&mut db), (0, 0, 0));
        assert_eq!(
            db.rebuild_search(&aliases).unwrap(),
            2,
            "one deleted, one added"
        );
        assert_eq!(db.search("again", 10).unwrap().tracks.len(), 1);
        assert!(db.search("hello again", 10).unwrap().albums.is_empty());
        // A new album: written, the others left alone.
        db.upsert_tracks(&[file("/m/Adele/30/01.flac", "30", "Easy on Me", 1)], 3)
            .unwrap();
        let (albums, links, _) = regroup(&mut db);
        assert_eq!((albums, links), (1, 1));
        assert_eq!(db.albums().unwrap().len(), 3);
        db.rebuild_search(&aliases).unwrap();
        assert_eq!(db.search("easy", 10).unwrap().tracks.len(), 1);
        // Playlists keep their rows, also when built for the first time.
        let mix = db.create_playlist("Mix", 4).unwrap();
        db.rebuild_search(&aliases).unwrap();
        assert_eq!(db.search("mix", 10).unwrap().playlists, [mix]);
    }

    #[test]
    fn related_artists_match_names_with_any_capitals() {
        let mut db = Database::open_in_memory().unwrap();
        for (key, name, mbid) in [
            ("edith", "Édith Piaf", "e"),
            ("aznavour", "Charles Aznavour", "c"),
        ] {
            db.conn
                .execute(
                    "INSERT INTO artist (key, name, sort_name, lookup_mbid) VALUES (?1, ?2, ?2, ?3)",
                    params![key, name, mbid],
                )
                .unwrap();
        }
        db.set_related_artists("e", &[("c".into(), 50), ("x".into(), 100)], 1)
            .unwrap();
        let related = db.related_artists().unwrap();
        // The key is lowercased as SongFeatures.artist is (all letters).
        let edith = related
            .get("édith piaf")
            .expect("found by its lowercase name");
        // Closeness against the best score, also an artist not in the library.
        assert_eq!(edith.get("charles aznavour"), Some(&0.5));
        assert_eq!(edith.len(), 1);
    }

    #[test]
    fn search_finds_substrings_short_words_and_aliases() {
        let mut db = Database::open_in_memory().unwrap();
        let mut korean = file("/m/TXT/A/01.flac", "The Name Chapter", "Sugar Rush Ride", 1);
        korean.tags.album_artist = Some("TOMORROW X TOGETHER".into());
        db.upsert_tracks(&[file("/m/Adele/25/01.flac", "25", "Hello", 1), korean], 1)
            .unwrap();
        rebuild(&mut db);
        db.create_playlist("Beyoncé Mix", 1).unwrap();
        let aliases = Aliases::parse_lyrembed("  TOMORROW X TOGETHER = TXT = 투모로우바이투게더\n");
        db.rebuild_search(&aliases).unwrap();

        let results = db.search("hell", 10).unwrap();
        assert_eq!(results.tracks.len(), 1);
        assert!(results.albums.is_empty());
        // Short words use LIKE: the album "25".
        assert_eq!(db.search("25", 10).unwrap().albums.len(), 1);
        // Aliases find the artist and its albums.
        let txt = db.search("투모로우", 10).unwrap();
        assert_eq!((txt.artists.len(), txt.albums.len()), (1, 1));
        assert_eq!(db.search("txt", 10).unwrap().artists.len(), 1);
        // Diacritics are ignored; all words must match.
        assert_eq!(db.search("beyonce mix", 10).unwrap().playlists.len(), 1);
        assert!(db.search("beyonce hello", 10).unwrap().is_empty());
        assert!(db.search("", 10).unwrap().is_empty());
        // Renamed: found by the new name only, once; deleted: gone.
        let mix = db.search("mix", 10).unwrap().playlists[0];
        db.rename_playlist(mix, "Road Trip", 2).unwrap();
        assert!(db.search("mix", 10).unwrap().playlists.is_empty());
        assert_eq!(db.search("road", 10).unwrap().playlists, [mix]);
        db.delete_playlist(mix).unwrap();
        assert!(db.search("road", 10).unwrap().playlists.is_empty());
    }

    #[test]
    fn album_genres_merge_case_variants() {
        let mut db = Database::open_in_memory().unwrap();
        let mut a = file("/m/X/A/01.flac", "A", "One", 1);
        a.tags.genres = vec!["K-Pop".into(), "Dance".into()];
        let mut b = file("/m/X/A/02.flac", "A", "Two", 2);
        b.tags.genres = vec!["K-POP".into()];
        let mut c = file("/m/X/A/03.flac", "A", "Three", 3);
        c.tags.genres = vec!["K-Pop".into()];
        db.upsert_tracks(&[a, b, c], 1).unwrap();
        rebuild(&mut db);
        let genres = db.album_genres().unwrap();
        assert_eq!(genres.len(), 1);
        assert_eq!(genres.values().next().unwrap(), &["K-Pop", "Dance"]);
    }

    #[test]
    fn analysis_is_shared_by_content_and_redone_after_changes() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(
            &[
                file("/m/A/1/01.flac", "1", "One", 1),
                file("/m/A/2/01.flac", "2", "One again", 1),
            ],
            1,
        )
        .unwrap();
        rebuild(&mut db);
        let ids = track_ids(&db);
        assert_eq!(db.tracks_to_analyze(10).unwrap().len(), 2);
        assert_eq!(db.analysis_progress().unwrap(), (0, 2));
        let result = crate::analysis::SongAnalysis {
            edges: Some(crate::analysis::Edges {
                lead_in_ms: 100,
                outro_ms: 170_000,
                end_ms: 178_000,
            }),
            features: vec![1.0, 2.0],
            features_version: crate::analysis::FEATURES_VERSION,
            bpm: Some(120.0),
            loudness: Some(-10.0),
        };
        // Both files have the same audio: one analysis serves both.
        db.set_content_key(ids["One"], "k1").unwrap();
        db.save_analysis("k1", Some(&result), 5).unwrap();
        assert!(db.has_analysis("k1").unwrap());
        db.set_content_key(ids["One again"], "k1").unwrap();
        assert!(db.tracks_to_analyze(10).unwrap().is_empty());
        assert_eq!(
            db.song_edges(ids["One again"]).unwrap().map(|e| e.outro_ms),
            Some(170_000)
        );
        assert_eq!(db.analysis_progress().unwrap(), (2, 2));
        // No ReplayGain tags: the measured loudness gives the gain.
        assert_eq!(db.measured_gain(ids["One"], false).unwrap(), Some(-8.0));
        let album = db.measured_gain(ids["One"], true).unwrap().unwrap();
        assert!((album + 8.0).abs() < 1e-9);
        let features = db.similarity_features().unwrap();
        assert!(
            features
                .iter()
                .all(|f| f.bpm == Some(120.0) && f.sound.is_some())
        );
        // A changed file loses its key and is looked at again.
        let mut changed = file("/m/A/1/01.flac", "1", "One", 1);
        changed.mtime += 10;
        db.upsert_tracks(&[changed], 2).unwrap();
        assert_eq!(db.tracks_to_analyze(10).unwrap().len(), 1);
        // A file that can't be analyzed isn't tried again.
        db.save_analysis("bad", None, 6).unwrap();
        assert!(db.has_analysis("bad").unwrap());
    }

    #[test]
    fn history_keeps_order_and_follows_removed_tracks() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(
            &[
                file("/m/Adele/25/01.flac", "25", "Hello", 1),
                file("/m/Adele/25/02.flac", "25", "Send My Love", 2),
            ],
            1,
        )
        .unwrap();
        rebuild(&mut db);
        let ids = track_ids(&db);
        let (hello, send) = (ids["Hello"], ids["Send My Love"]);
        db.add_history(hello, 10).unwrap();
        db.add_history(send, 11).unwrap();
        db.add_history(hello, 12).unwrap();
        assert_eq!(db.history(10).unwrap(), [hello, send, hello]);
        assert_eq!(db.history(2).unwrap(), [send, hello]);
        db.delete_tracks(&[PathBuf::from("/m/Adele/25/01.flac")])
            .unwrap();
        assert_eq!(db.history(10).unwrap(), [send]);
    }

    #[test]
    fn queue_round_trip() {
        let mut db = Database::open_in_memory().unwrap();
        assert_eq!(db.load_queue().unwrap(), None);
        let snapshot = QueueSnapshot {
            items: vec![
                SnapshotItem {
                    track_id: 5,
                    origin: Origin::Context,
                    unshuffled_position: Some(1),
                },
                SnapshotItem {
                    track_id: 7,
                    origin: Origin::User,
                    unshuffled_position: Some(0),
                },
            ],
            current: Some(1),
            position_ms: 12_345,
            shuffle: true,
            repeat: RepeatMode::All,
            context_album: Some(3),
        };
        db.save_queue(&snapshot).unwrap();
        db.save_queue(&snapshot).unwrap();
        assert_eq!(db.load_queue().unwrap(), Some(snapshot));
    }
}
