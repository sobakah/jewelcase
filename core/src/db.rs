//! The library database: SQLite via rusqlite (CONCEPT.md §3.2, §6.1).
//!
//! The UI and the scanner each open their own connection; WAL mode lets the
//! UI read while a scan writes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row, params};
use rusqlite_migration::{M, Migrations};

use crate::library::TrackRecord;
use crate::library::artists::{AlbumRole, ArtistInfo};
use crate::library::grouping::AlbumInfo;
use crate::library::tags::{Lyrics, TrackTags};
use crate::library::titles::ReleaseType;
use crate::queue::{Origin, QueueSnapshot, RepeatMode};

const MIGRATIONS: &[M<'static>] = &[
    M::up(include_str!("db/001_library.sql")),
    M::up(include_str!("db/002_barcode_and_album_order.sql")),
    M::up(include_str!("db/003_artists.sql")),
];

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

/// A file found by the scanner, with its stat data and tags.
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
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Migrations::from_slice(MIGRATIONS).to_latest(&mut conn)?;
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
    pub fn replace_library(&mut self, albums: &[AlbumInfo], artists: &[ArtistInfo]) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut album_ids: HashMap<&str, i64> = HashMap::new();
        let mut artist_ids = HashSet::new();
        {
            let mut upsert = tx.prepare_cached(UPSERT_ALBUM)?;
            let mut link = tx.prepare_cached(
                "UPDATE track SET album_id = ?1, album_position = ?3, album_disc = ?4 WHERE id = ?2",
            )?;
            for album in albums {
                let id: i64 = upsert.query_row(
                    params![
                        album.key,
                        album.group_key,
                        album.title,
                        album.sort_title,
                        album.edition,
                        album.artist_credit,
                        album.artist_sort,
                        album.year,
                        album.release_type.as_str(),
                        album.soundtrack,
                        album.compilation,
                        album.mb_album_id,
                        album.mb_release_group_id,
                        album.track_ids.len() as i64,
                        album.duration_ms as i64,
                        album.cover_key,
                        album.added,
                    ],
                    |row| row.get(0),
                )?;
                album_ids.insert(&album.key, id);
                for (position, track_id) in album.track_ids.iter().enumerate() {
                    let disc = album.track_discs.get(position).copied();
                    link.execute(params![id, track_id, position as i64, disc])?;
                }
            }

            tx.execute("DELETE FROM album_artist", [])?;
            tx.execute("DELETE FROM track_artist", [])?;
            let mut upsert_artist = tx.prepare_cached(UPSERT_ARTIST)?;
            let mut album_artist = tx.prepare_cached(
                "INSERT OR IGNORE INTO album_artist (album_id, artist_id, role, position)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut track_artist = tx.prepare_cached(
                "INSERT OR IGNORE INTO track_artist (track_id, artist_id) VALUES (?1, ?2)",
            )?;
            for artist in artists {
                let id: i64 = upsert_artist.query_row(
                    params![artist.key, artist.name, artist.sort_name, artist.mbid],
                    |row| row.get(0),
                )?;
                artist_ids.insert(id);
                for (album_key, role, position) in &artist.albums {
                    if let Some(album_id) = album_ids.get(album_key.as_str()) {
                        album_artist.execute(params![
                            album_id,
                            id,
                            role.as_str(),
                            *position as i64
                        ])?;
                    }
                }
                for track_id in &artist.appears_on {
                    track_artist.execute(params![track_id, id])?;
                }
            }
        }
        let kept: HashSet<i64> = album_ids.into_values().collect();
        delete_except(&tx, "album", &kept)?;
        delete_except(&tx, "artist", &artist_ids)?;
        tx.commit()?;
        Ok(())
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

    pub fn tracks(&self, ids: &[i64]) -> Result<HashMap<i64, Track>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {TRACK_COLUMNS}, {TRACK_ALBUM_COLUMNS} FROM track t
               LEFT JOIN album a ON a.id = t.album_id
             WHERE t.id = ?1"
        ))?;
        let mut tracks = HashMap::new();
        for id in ids {
            if let Some(track) = stmt.query_row([id], track).optional()? {
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

const UPSERT_TRACK: &str = "INSERT INTO track (path, mtime, size, added, title, artist, album,
    album_artist, album_artists, album_artist_sort, disc, number, year, genres, composer,
    mb_album_id, mb_release_group_id, mb_track_id, mb_album_artist_ids, release_comment,
    release_type, compilation, isrc, bpm, lyrics_synced, lyrics_excerpt, replaygain_track,
    replaygain_album, duration_ms, format, bitrate, sample_rate, bit_depth, channels, cover_key,
    barcode)
  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
    ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36)
  ON CONFLICT(path) DO UPDATE SET mtime = excluded.mtime, size = excluded.size,
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

const UPSERT_ARTIST: &str =
    "INSERT INTO artist (key, name, sort_name, mbid) VALUES (?1, ?2, ?3, ?4)
  ON CONFLICT(key) DO UPDATE SET name = excluded.name, sort_name = excluded.sort_name,
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
    ar.portrait_attribution, ar.custom_portrait_key";

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

    fn file(path: &str, album: &str, title: &str, number: u32) -> ScannedFile {
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

    fn rebuild(db: &mut Database) {
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
