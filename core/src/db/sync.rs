//! The sync with Jewelcase Mobile through sync files (CONCEPT.md §6.4; the format is Jewelcase
//! Mobile's `docs/SYNC.md`): every device writes `jewelcase-sync-<device>.json` into a folder
//! both reach and reads the others'. Merged: the user's choices for Spotify songs and LRCLIB's
//! lyrics by the latest time per song, MusicBrainz's ISRCs by release, plays by id, local
//! playlists by UUID (the latest change wins whole; deletions are kept). Jewelcase writes
//! lyrembed's alias list too. This is the pure part: the app reads and writes the files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use super::{Database, Result};

/// The sync files' format and version this reads and writes.
const FORMAT: &str = "jewelcase-sync";
const VERSION: i64 = 1;
pub const PREFIX: &str = "jewelcase-sync-";
pub const SUFFIX: &str = ".json";
/// A song of the phone with this close a length is the same.
const SAME_LENGTH_MS: i64 = 3_000;

/// The file name of a device's sync file.
pub fn file_name(device: &str) -> String {
    format!("{PREFIX}{device}{SUFFIX}")
}

/// Now, in Unix milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// This installation in the sync.
#[derive(Debug, Clone)]
pub struct Device<'a> {
    /// Its UUID, made once.
    pub id: &'a str,
    pub name: &'a str,
}

/// Another device's file in the folder; `synced`: read, not turned off in Preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncDevice {
    pub id: String,
    pub name: String,
    /// When it was written, Unix ms.
    pub written: i64,
    pub synced: bool,
}

/// What a sync did: the other devices' files found (newest first) and the files skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncResult {
    pub devices: Vec<SyncDevice>,
    pub skipped: Vec<String>,
}

impl SyncResult {
    /// The names of the devices synced with.
    pub fn synced(&self) -> Vec<&str> {
        self.devices
            .iter()
            .filter(|d| d.synced)
            .map(|d| d.name.as_str())
            .collect()
    }
}

impl Database {
    /// Merges the other devices' files (`files`: name and text; this device's own is passed
    /// over) but those of the devices `off` (their IDs, turned off in Preferences; still
    /// listed), and returns this device's file. `folders`: the library folders, which paths
    /// are relative to in the files; `aliases`: lyrembed's list to pass on.
    pub fn sync(
        &mut self,
        device: &Device,
        files: &[(String, String)],
        off: &[String],
        folders: &[PathBuf],
        aliases: Option<&str>,
        now_ms: i64,
    ) -> Result<(String, SyncResult)> {
        let own = file_name(device.id);
        let mut result = SyncResult::default();
        let mut others = Vec::new();
        for (name, text) in files {
            if *name == own {
                continue;
            }
            match read(text).filter(|f| str_of(f, "device") != Some(device.id)) {
                Some(file) => others.push(file),
                None => result.skipped.push(name.clone()),
            }
        }
        others.sort_by_key(|f| f.get("written").and_then(Value::as_i64).unwrap_or(0));
        self.give_sync_ids()?;
        for file in &others {
            let id = str_of(file, "device").unwrap_or_default();
            let name = str_of(file, "name").filter(|n| !n.is_empty()).unwrap_or(id);
            let synced = !off.iter().any(|o| o == id);
            result.devices.push(SyncDevice {
                id: id.to_owned(),
                name: name.to_owned(),
                written: file.get("written").and_then(Value::as_i64).unwrap_or(0),
                synced,
            });
            if synced {
                self.merge(file, device, folders)?;
            }
        }
        result.devices.reverse();
        let text = self.snapshot(device, folders, aliases, now_ms)?.to_string();
        Ok((text, result))
    }

    fn merge(&mut self, file: &Value, device: &Device, folders: &[PathBuf]) -> Result<()> {
        for choice in list(file, "choices") {
            self.merge_choice(choice, folders)?;
        }
        for release in list(file, "isrcs") {
            self.merge_release(release)?;
        }
        for lyrics in list(file, "lyrics") {
            self.merge_lyrics(lyrics)?;
        }
        self.merge_plays(list(file, "plays"), device, folders)?;
        let mut playlists_changed = false;
        for playlist in list(file, "playlists") {
            playlists_changed |= self.merge_playlist(playlist, folders)?;
        }
        if playlists_changed {
            self.relink_playlists()?;
        }
        Ok(())
    }

    // Choices ------------------------------------------------------------------------------

    /// A choice for a Spotify song: applied to every playlist entry of the song when newer than
    /// this side's (Jewelcase keeps choices per entry).
    fn merge_choice(&mut self, choice: &Value, folders: &[PathBuf]) -> Result<()> {
        let Some(uri) = str_of(choice, "uri") else {
            return Ok(());
        };
        let at = choice.get("at").and_then(Value::as_i64).unwrap_or(0);
        let mine: i64 = self.conn.query_row(
            "SELECT MAX(COALESCE((SELECT MAX(choice_at) FROM playlist_item WHERE spotify_uri = ?1), 0),
                        COALESCE((SELECT at FROM forgotten_choice WHERE uri = ?1), 0))",
            [uri],
            |row| row.get(0),
        )?;
        if at <= mine {
            return Ok(());
        }
        match str_of(choice, "choice") {
            Some("spotify") => {
                self.conn.execute(
                    "UPDATE playlist_item SET local_track_id = NULL, match_state = 'user_pinned_spotify',
                       suggested_track_id = NULL, choice_at = ?2 WHERE spotify_uri = ?1",
                    params![uri, at],
                )?;
                self.conn
                    .execute("DELETE FROM forgotten_choice WHERE uri = ?1", [uri])?;
            }
            Some("file") => {
                // A file this side doesn't have: nothing to play from here.
                let Some(track) = choice
                    .get("file")
                    .map(|f| self.find_track(f, folders))
                    .transpose()?
                    .flatten()
                else {
                    return Ok(());
                };
                self.conn.execute(
                    "UPDATE playlist_item SET local_track_id = ?2, match_state = 'user_matched',
                       suggested_track_id = NULL, choice_at = ?3 WHERE spotify_uri = ?1",
                    params![uri, track, at],
                )?;
                self.conn
                    .execute("DELETE FROM forgotten_choice WHERE uri = ?1", [uri])?;
            }
            _ => {
                self.conn.execute(
                    "UPDATE playlist_item SET local_track_id = NULL, match_state = 'spotify_only',
                       suggested_track_id = NULL, choice_at = NULL
                     WHERE spotify_uri = ?1 AND match_state IN ('user_matched', 'user_pinned_spotify')",
                    [uri],
                )?;
                self.conn.execute(
                    "INSERT OR REPLACE INTO forgotten_choice (uri, at) VALUES (?1, ?2)",
                    params![uri, at],
                )?;
            }
        }
        Ok(())
    }

    // ISRCs and lyrics -----------------------------------------------------------------------

    fn merge_release(&mut self, release: &Value) -> Result<()> {
        let Some(id) = str_of(release, "release") else {
            return Ok(());
        };
        let checked = release.get("checked").and_then(Value::as_i64).unwrap_or(0);
        let mine: Option<i64> = self
            .conn
            .query_row(
                "SELECT checked FROM mb_isrc_release WHERE release = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if mine.is_some_and(|m| checked <= m) {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM mb_isrc WHERE release = ?1", [id])?;
        for entry in list(release, "isrcs") {
            let (Some(disc), Some(number), Some(isrc)) = (
                entry.get(0).and_then(Value::as_i64),
                entry.get(1).and_then(Value::as_i64),
                entry
                    .get(2)
                    .and_then(Value::as_str)
                    .filter(|i| i.len() == 12),
            ) else {
                continue;
            };
            tx.execute(
                "INSERT OR IGNORE INTO mb_isrc (release, disc, number, isrc) VALUES (?1, ?2, ?3, ?4)",
                params![id, disc, number, isrc.to_uppercase()],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO mb_isrc_release (release, checked) VALUES (?1, ?2)",
            params![id, checked],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn merge_lyrics(&mut self, lyrics: &Value) -> Result<()> {
        let Some(uri) = str_of(lyrics, "uri") else {
            return Ok(());
        };
        let checked = lyrics.get("checked").and_then(Value::as_i64).unwrap_or(0);
        let mine: Option<i64> = self
            .conn
            .query_row(
                "SELECT checked FROM online_lyrics WHERE uri = ?1",
                [uri],
                |row| row.get(0),
            )
            .optional()?;
        if mine.is_some_and(|m| checked <= m) {
            return Ok(());
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO online_lyrics (uri, checked, found, synced, plain, instrumental)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                uri,
                checked,
                lyrics.get("found").and_then(Value::as_bool).unwrap_or(false),
                str_of(lyrics, "synced"),
                str_of(lyrics, "plain"),
                lyrics.get("instrumental").and_then(Value::as_bool).unwrap_or(false),
            ],
        )?;
        Ok(())
    }

    // Plays ------------------------------------------------------------------------------------

    /// The other device's plays this side hasn't got (nor deleted), kept with their id.
    fn merge_plays(&mut self, plays: &[Value], device: &Device, folders: &[PathBuf]) -> Result<()> {
        let mine = format!("{}:", device.id);
        let have: HashSet<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT remote_id FROM play_log WHERE remote_id IS NOT NULL
                 UNION SELECT remote_id FROM skipped_play",
            )?;
            stmt.query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for play in plays {
            let Some(id) =
                str_of(play, "id").filter(|id| !have.contains(*id) && !id.starts_with(&mine))
            else {
                continue;
            };
            let track = match str_of(play, "path") {
                Some(path) => self.track_at(path, folders)?,
                None => None,
            };
            self.conn.execute(
                "INSERT OR IGNORE INTO play_log (played, ms_played, track_id, title, artist, album,
                   album_artist, genre, remote_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    play.get("played").and_then(Value::as_i64).unwrap_or(0),
                    play.get("msPlayed").and_then(Value::as_i64).unwrap_or(0),
                    track.unwrap_or(0),
                    str_of(play, "title").unwrap_or_default(),
                    str_of(play, "artist"),
                    str_of(play, "album"),
                    str_of(play, "albumArtist"),
                    str_of(play, "genre"),
                    id,
                ],
            )?;
        }
        Ok(())
    }

    // Playlists --------------------------------------------------------------------------------

    /// Every local playlist gets its UUID before it goes into a file.
    fn give_sync_ids(&mut self) -> Result<()> {
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM playlist WHERE source = 'local' AND sync_id IS NULL")?;
            stmt.query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for id in ids {
            self.conn.execute(
                "UPDATE playlist SET sync_id = ?2 WHERE id = ?1",
                params![id, uuid()],
            )?;
        }
        Ok(())
    }

    /// A playlist as the other device has it, when newer than this side's: replaced whole,
    /// deleted, or new. Whether anything changed.
    fn merge_playlist(&mut self, playlist: &Value, folders: &[PathBuf]) -> Result<bool> {
        let Some(sync_id) = str_of(playlist, "id") else {
            return Ok(false);
        };
        let modified = playlist
            .get("modified")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let local: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT id, modified FROM playlist WHERE sync_id = ?1",
                [sync_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let deleted_at: Option<i64> = self
            .conn
            .query_row(
                "SELECT at FROM playlist_deleted WHERE sync_id = ?1",
                [sync_id],
                |row| row.get(0),
            )
            .optional()?;
        // Jewelcase keeps a playlist's times in seconds, the files in ms.
        let mine = local
            .map(|(_, m)| m * 1000)
            .max(deleted_at)
            .unwrap_or(i64::MIN);
        if modified <= mine {
            return Ok(false);
        }
        if playlist
            .get("deleted")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            if let Some((id, _)) = local {
                self.delete_playlist(id)?;
            }
            self.conn.execute(
                "INSERT OR REPLACE INTO playlist_deleted (sync_id, at) VALUES (?1, ?2)",
                params![sync_id, modified],
            )?;
            return Ok(true);
        }
        // Back from another device: no longer deleted here (or its file says both).
        self.conn
            .execute("DELETE FROM playlist_deleted WHERE sync_id = ?1", [sync_id])?;
        let name = str_of(playlist, "name").unwrap_or_default();
        let seconds = modified / 1000;
        let id = match local {
            Some((id, _)) => {
                self.rename_playlist(id, name, seconds)?;
                id
            }
            None => {
                let id = self.create_playlist(name, seconds)?;
                self.conn.execute(
                    "UPDATE playlist SET sync_id = ?2, created = ?3 WHERE id = ?1",
                    params![
                        id,
                        sync_id,
                        playlist
                            .get("created")
                            .and_then(Value::as_i64)
                            .unwrap_or(modified)
                            / 1000
                    ],
                )?;
                id
            }
        };
        // Linked with a Spotify playlist (published on either side): this playlist stands for it,
        // its copy of the Spotify playlist goes (unless its own changes wait to be sent), and the
        // next Spotify sync reads it in (no snapshot). A file without the link keeps this one's.
        let mut linked: Option<String> = self.conn.query_row(
            "SELECT spotify_id FROM playlist WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        if let Some(spotify_id) =
            str_of(playlist, "spotify").filter(|s| linked.as_deref() != Some(*s))
        {
            let mirror: Option<(i64, i64)> = self
                .conn
                .query_row(
                    "SELECT id, (SELECT COUNT(*) FROM spotify_outbox o WHERE o.playlist_id = playlist.id)
                     FROM playlist WHERE spotify_id = ?1 AND source = 'spotify'",
                    [spotify_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let free = match mirror {
                Some((_, waiting)) if waiting > 0 => false,
                Some((mirror, _)) => {
                    self.delete_playlist(mirror)?;
                    true
                }
                None => true,
            };
            if free {
                self.conn.execute(
                    "UPDATE playlist SET spotify_id = ?2, snapshot_id = NULL WHERE id = ?1",
                    params![id, spotify_id],
                )?;
                linked = Some(spotify_id.to_owned());
            }
        }
        let songs = list(playlist, "songs");
        let mut found = Vec::with_capacity(songs.len());
        for song in songs {
            let track = match str_of(song, "path") {
                Some(_) => self.find_track(song, folders)?,
                None => None,
            };
            found.push(track);
        }
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM playlist_item WHERE playlist_id = ?1", [id])?;
        for (position, (song, track)) in songs.iter().zip(found).enumerate() {
            let artist = song
                .get("artists")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|a| !a.is_empty());
            let path = str_of(song, "path").map(|relative| absolute(relative, folders));
            let uri = str_of(song, "uri");
            // A library song this side has, a Spotify song, or a library song it hasn't got.
            // In a linked playlist a song keeps its Spotify URI also when it plays from a file.
            let keep_uri = linked.is_some() && uri.is_some();
            let state = match (track, uri) {
                (Some(_), Some(_)) if keep_uri => "matched",
                (Some(_), _) => "local",
                (None, Some(_)) => "spotify_only",
                (None, None) => "unavailable",
            };
            tx.execute(
                "INSERT INTO playlist_item (playlist_id, position, local_track_id, spotify_uri, isrc,
                   title, artist, album, duration_ms, path, image_url, match_state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    id,
                    position as i64,
                    track,
                    if track.is_some() && !keep_uri { None } else { uri },
                    str_of(song, "isrc"),
                    str_of(song, "title"),
                    artist,
                    str_of(song, "album"),
                    song.get("durationMs").and_then(Value::as_i64),
                    path,
                    str_of(song, "imageUrl"),
                    state,
                ],
            )?;
        }
        tx.execute(
            "UPDATE playlist SET modified = ?2 WHERE id = ?1",
            params![id, seconds],
        )?;
        tx.commit()?;
        Ok(true)
    }

    // Finding the other device's songs ---------------------------------------------------------

    /// This side's library song at a path relative to one of the library folders.
    fn track_at(&self, relative: &str, folders: &[PathBuf]) -> Result<Option<i64>> {
        for folder in folders {
            let path = folder.join(relative);
            let found: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM track WHERE path = ?1",
                    [path.to_string_lossy()],
                    |row| row.get(0),
                )
                .optional()?;
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }

    /// The other device's song in this library: by its path, else its ISRC (one song), else
    /// title, artist and length (one song).
    fn find_track(&self, song: &Value, folders: &[PathBuf]) -> Result<Option<i64>> {
        if let Some(path) = str_of(song, "path")
            && let Some(found) = self.track_at(path, folders)?
        {
            return Ok(Some(found));
        }
        if let Some(isrc) = str_of(song, "isrc") {
            let ids = self.ids(
                "SELECT id FROM track WHERE upper(isrc) = upper(?1)",
                params![isrc],
            )?;
            if let [only] = ids[..] {
                return Ok(Some(only));
            }
        }
        let artist = str_of(song, "artist").map(str::to_owned).or_else(|| {
            song.get("artists")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        let (Some(title), Some(artist)) = (str_of(song, "title"), artist) else {
            return Ok(None);
        };
        let length = song.get("durationMs").and_then(Value::as_i64);
        let ids = self.ids(
            "SELECT id FROM track WHERE title = ?1 COLLATE NOCASE AND artist = ?2 COLLATE NOCASE
               AND (?3 IS NULL OR abs(duration_ms - ?3) <= ?4)",
            params![title, artist, length, SAME_LENGTH_MS],
        )?;
        Ok(if let [only] = ids[..] {
            Some(only)
        } else {
            None
        })
    }

    fn ids(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(sql)?;
        Ok(stmt
            .query_map(params, |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    // Writing ----------------------------------------------------------------------------------

    fn snapshot(
        &self,
        device: &Device,
        folders: &[PathBuf],
        aliases: Option<&str>,
        now_ms: i64,
    ) -> Result<Value> {
        let relative = |path: Option<String>| path.and_then(|p| relative_to(&p, folders));

        // Choices: the latest per Spotify song, then the forgotten ones.
        let mut choices: HashMap<String, Value> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT i.spotify_uri, i.match_state, i.choice_at, t.path, t.isrc, t.title, t.artist,
                   t.album, t.duration_ms
                 FROM playlist_item i LEFT JOIN track t ON t.id = i.local_track_id
                 WHERE i.spotify_uri IS NOT NULL
                   AND i.match_state IN ('user_matched', 'user_pinned_spotify')
                 ORDER BY COALESCE(i.choice_at, 0)",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                ))
            })?;
            for row in rows {
                let (uri, state, at, path, isrc, title, artist, album, length) = row?;
                let value = if state == "user_pinned_spotify" {
                    json!({ "uri": uri, "choice": "spotify", "at": at.unwrap_or(0) })
                } else {
                    let Some(path) = relative(path) else { continue };
                    json!({ "uri": uri, "choice": "file", "at": at.unwrap_or(0), "file": {
                        "path": path, "isrc": isrc, "title": title, "artist": artist,
                        "album": album, "durationMs": length } })
                };
                choices.insert(uri, value);
            }
        }
        let forgotten: Vec<(String, i64)> = {
            let mut stmt = self.conn.prepare("SELECT uri, at FROM forgotten_choice")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (uri, at) in forgotten {
            choices
                .entry(uri.clone())
                .or_insert(json!({ "uri": uri, "choice": null, "at": at }));
        }

        let isrcs: Vec<Value> = {
            let mut by_release: HashMap<String, Vec<Value>> = HashMap::new();
            let mut stmt = self
                .conn
                .prepare("SELECT release, disc, number, isrc FROM mb_isrc")?;
            for row in stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })? {
                let (release, disc, number, isrc) = row?;
                by_release
                    .entry(release)
                    .or_default()
                    .push(json!([disc, number, isrc]));
            }
            let mut stmt = self
                .conn
                .prepare("SELECT release, checked FROM mb_isrc_release")?;
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|(release, checked)| {
                let list = by_release.remove(&release).unwrap_or_default();
                json!({ "release": release, "checked": checked, "isrcs": list })
            })
            .collect()
        };

        let lyrics: Vec<Value> = {
            let mut stmt = self.conn.prepare(
                "SELECT uri, checked, found, synced, plain, instrumental FROM online_lyrics",
            )?;
            stmt.query_map([], |row| {
                Ok(json!({
                    "uri": row.get::<_, String>(0)?, "checked": row.get::<_, i64>(1)?,
                    "found": row.get::<_, bool>(2)?, "synced": row.get::<_, Option<String>>(3)?,
                    "plain": row.get::<_, Option<String>>(4)?, "instrumental": row.get::<_, bool>(5)?
                }))
            })?
            .collect::<rusqlite::Result<_>>()?
        };

        // This side's own plays (not those read), with the song's path or Spotify URI.
        let plays: Vec<Value> = {
            let mut stmt = self.conn.prepare(
                "SELECT l.id, l.played, l.ms_played, l.title, l.artist, l.album, l.album_artist,
                   l.genre, t.path, s.uri
                 FROM play_log l
                 LEFT JOIN track t ON l.track_id > 0 AND t.id = l.track_id
                 LEFT JOIN stream_track s ON l.track_id < 0 AND s.id = -l.track_id
                 WHERE l.remote_id IS NULL ORDER BY l.id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            })?;
            let mut plays = Vec::new();
            for row in rows {
                let (id, played, ms, title, artist, album, album_artist, genre, path, uri) = row?;
                plays.push(json!({
                    "id": format!("{}:{id}", device.id), "played": played, "msPlayed": ms,
                    "title": title, "artist": artist, "album": album, "albumArtist": album_artist,
                    "genre": genre, "uri": uri, "path": relative(path)
                }));
            }
            plays
        };

        // Local playlists (Spotify's aren't synced: both read them from Spotify) and deletions.
        let mut playlists: Vec<Value> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, sync_id, name, created, modified, spotify_id FROM playlist
                 WHERE source = 'local' AND sync_id IS NOT NULL",
            )?;
            let rows: Vec<(i64, String, String, i64, i64, Option<String>)> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            for (id, sync_id, name, created, modified, spotify_id) in rows {
                let mut items = self.conn.prepare(
                    "SELECT i.spotify_uri, COALESCE(t.path, i.path), COALESCE(t.isrc, i.isrc),
                       COALESCE(t.title, i.title), COALESCE(t.artist, i.artist), COALESCE(t.album, i.album),
                       COALESCE(t.duration_ms, i.duration_ms), i.image_url
                     FROM playlist_item i LEFT JOIN track t ON t.id = i.local_track_id
                     WHERE i.playlist_id = ?1 ORDER BY i.position",
                )?;
                let songs: Vec<Value> = items
                    .query_map([id], |row| {
                        let path: Option<String> = row.get(1)?;
                        let artist: Option<String> = row.get(4)?;
                        Ok(json!({
                            "uri": row.get::<_, Option<String>>(0)?,
                            "path": path.and_then(|p| relative_to(&p, folders).or(Some(p))),
                            "isrc": row.get::<_, Option<String>>(2)?,
                            "title": row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                            "artists": artist.map(|a| vec![a]).unwrap_or_default(),
                            "album": row.get::<_, Option<String>>(5)?,
                            "durationMs": row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                            "imageUrl": row.get::<_, Option<String>>(7)?
                        }))
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                let mut playlist = json!({
                    "id": sync_id, "name": name, "created": created * 1000, "modified": modified * 1000,
                    "deleted": false, "songs": songs
                });
                if let Some(spotify_id) = spotify_id {
                    playlist["spotify"] = Value::String(spotify_id);
                }
                playlists.push(playlist);
            }
            let mut stmt = self.conn.prepare(
                "SELECT sync_id, at FROM playlist_deleted d
                     WHERE NOT EXISTS (SELECT 1 FROM playlist p WHERE p.sync_id = d.sync_id)",
            )?;
            for row in stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })? {
                let (id, at) = row?;
                playlists.push(json!({ "id": id, "name": "", "created": at, "modified": at,
                    "deleted": true, "songs": [] }));
            }
        }

        let mut file = json!({
            "format": FORMAT, "version": VERSION, "device": device.id, "name": device.name,
            "written": now_ms, "choices": choices.into_values().collect::<Vec<_>>(),
            "isrcs": isrcs, "lyrics": lyrics, "plays": plays, "playlists": playlists
        });
        if let Some(aliases) = aliases.filter(|a| !a.trim().is_empty()) {
            file["aliases"] = Value::String(aliases.to_owned());
        }
        Ok(file)
    }
}

/// A sync file, or None when it isn't one or is of a newer version.
fn read(text: &str) -> Option<Value> {
    let file: Value = serde_json::from_str(text).ok()?;
    (str_of(&file, "format") == Some(FORMAT)
        && file
            .get("version")
            .and_then(Value::as_i64)
            .is_some_and(|v| v <= VERSION)
        && str_of(&file, "device").is_some())
    .then_some(file)
}

fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// A path below one of the library folders, with `/`.
fn relative_to(path: &str, folders: &[PathBuf]) -> Option<String> {
    folders.iter().find_map(|folder| {
        Path::new(path)
            .strip_prefix(folder)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
    })
}

/// A relative path under the first library folder (kept as given without one).
fn absolute(relative: &str, folders: &[PathBuf]) -> String {
    match folders.first() {
        Some(folder) => folder.join(relative).to_string_lossy().into_owned(),
        None => relative.to_owned(),
    }
}

/// A random UUID (version 4) for a playlist.
fn uuid() -> String {
    let bytes: [u8; 16] = rand::random();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-4{}-{:x}{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        (bytes[8] & 0x3) | 0x8,
        &hex[17..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::{file, rebuild};

    const ME: Device = Device {
        id: "desk",
        name: "Jewelcase",
    };

    fn db_with_songs() -> Database {
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
        db
    }

    fn id_of(db: &Database, path: &str) -> i64 {
        db.conn
            .query_row("SELECT id FROM track WHERE path = ?1", [path], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn phone(body: Value) -> (String, String) {
        let mut file = json!({ "format": FORMAT, "version": 1, "device": "phone", "name": "Jewelcase Mobile",
            "written": 5_000 });
        for (key, value) in body.as_object().unwrap() {
            file[key] = value.clone();
        }
        (file_name("phone"), file.to_string())
    }

    fn sync(db: &mut Database, files: &[(String, String)]) -> (Value, SyncResult) {
        sync_but(db, files, &[])
    }

    fn sync_but(
        db: &mut Database,
        files: &[(String, String)],
        off: &[String],
    ) -> (Value, SyncResult) {
        let (text, result) = db
            .sync(
                &ME,
                files,
                off,
                &[PathBuf::from("/m")],
                Some("TXT = TOMORROW X TOGETHER"),
                9_000,
            )
            .unwrap();
        (serde_json::from_str(&text).unwrap(), result)
    }

    #[test]
    fn a_playlist_published_on_the_phone_takes_the_place_of_its_spotify_copy() {
        let mut db = db_with_songs();
        // The Spotify playlist as this side read it: a copy of its own.
        db.conn
            .execute(
                "INSERT INTO playlist (name, created, modified, source, spotify_id, snapshot_id)
                 VALUES ('Mixed', 1, 1, 'spotify', 'sp', 's1')",
                [],
            )
            .unwrap();
        let files = [phone(
            json!({ "playlists": [{ "id": "p1", "name": "Mixed", "created": 1000,
            "modified": 40000, "deleted": false, "spotify": "sp", "songs": [
                { "uri": "spotify:track:hello", "path": "Adele/25/01.flac", "title": "Hello",
                  "artists": ["Adele"], "durationMs": 295000 },
                { "path": "Nowhere/x.flac", "title": "Only on the Phone", "artists": ["Someone"],
                  "durationMs": 1000 }
            ] }] }),
        )];
        let (mine, _) = sync(&mut db, &files);
        // One playlist for "sp": the phone's, linked, to be read from Spotify again.
        let rows: Vec<(String, Option<String>, Option<String>)> = db
            .conn
            .prepare("SELECT source, sync_id, snapshot_id FROM playlist WHERE spotify_id = 'sp'")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, [("local".to_owned(), Some("p1".to_owned()), None)]);
        // The song keeps its URI though it plays from the file.
        let (uri, state): (Option<String>, String) = db
            .conn
            .query_row(
                "SELECT spotify_uri, match_state FROM playlist_item WHERE title = 'Hello'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (uri.as_deref(), state.as_str()),
            (Some("spotify:track:hello"), "matched")
        );
        // Written back with its link.
        let written = mine["playlists"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "p1")
            .unwrap();
        assert_eq!(written["spotify"], "sp");
    }

    #[test]
    fn a_playlist_back_from_another_device_is_written_alive_only() {
        let mut db = db_with_songs();
        db.conn
            .execute(
                "INSERT INTO playlist_deleted (sync_id, at) VALUES ('p1', 30000)",
                [],
            )
            .unwrap();
        let files = [phone(json!({ "playlists": [{ "id": "p1", "name": "Back",
            "created": 1000, "modified": 40000, "deleted": false, "songs": [] }] }))];
        let (mine, _) = sync(&mut db, &files);
        let listed: Vec<(&str, bool)> = mine["playlists"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["id"] == "p1")
            .map(|p| (p["id"].as_str().unwrap(), p["deleted"].as_bool().unwrap()))
            .collect();
        assert_eq!(listed, [("p1", false)]);
        let markers: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM playlist_deleted", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(markers, 0);
    }

    #[test]
    fn a_device_turned_off_is_listed_but_not_read() {
        let mut db = db_with_songs();
        let play = |id: &str| {
            json!({ "id": id, "played": 200, "msPlayed": 60_000, "title": "Send My Love",
            "artist": "Adele", "path": "Adele/25/02.flac" })
        };
        let mut dev = json!({ "format": FORMAT, "version": 1, "device": "dev",
            "name": "Jewelcase Mobile Dev", "written": 6_000, "plays": [play("dev:1")] });
        let files = [
            phone(json!({ "plays": [play("phone:1")] })),
            (file_name("dev"), dev.take().to_string()),
        ];
        let (_, result) = sync_but(&mut db, &files, &["dev".to_owned()]);
        assert_eq!(
            result.devices,
            [
                SyncDevice {
                    id: "dev".into(),
                    name: "Jewelcase Mobile Dev".into(),
                    written: 6_000,
                    synced: false
                },
                SyncDevice {
                    id: "phone".into(),
                    name: "Jewelcase Mobile".into(),
                    written: 5_000,
                    synced: true
                },
            ]
        );
        assert_eq!(result.synced(), ["Jewelcase Mobile"]);
        let remote: Vec<String> = db
            .conn
            .prepare("SELECT remote_id FROM play_log WHERE remote_id IS NOT NULL")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(remote, ["phone:1"]);
    }

    #[test]
    fn plays_come_in_once_and_go_out_as_this_sides_own() {
        let mut db = db_with_songs();
        let hello = id_of(&db, "/m/Adele/25/01.flac");
        db.log_play(hello, 100, 120_000).unwrap();
        db.conn
            .execute("INSERT INTO skipped_play VALUES ('phone:9')", [])
            .unwrap();
        let play = |id: &str| {
            json!({ "id": id, "played": 200, "msPlayed": 60_000, "title": "Send My Love",
            "artist": "Adele", "path": "Adele/25/02.flac" })
        };
        let files = [phone(
            json!({ "plays": [play("phone:1"), play("phone:9"), play("desk:1")] }),
        )];
        let (mine, result) = sync(&mut db, &files);
        sync(&mut db, &files);
        assert_eq!(result.synced(), ["Jewelcase Mobile"]);
        let remote: Vec<(String, i64)> = db
            .conn
            .prepare("SELECT remote_id, track_id FROM play_log WHERE remote_id IS NOT NULL")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            remote,
            [("phone:1".to_owned(), id_of(&db, "/m/Adele/25/02.flac"))]
        );
        let plays = mine["plays"].as_array().unwrap();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0]["path"], "Adele/25/01.flac");
        assert_eq!(mine["aliases"], "TXT = TOMORROW X TOGETHER");
        // Deleting the statistics keeps the phone's plays from coming back.
        db.delete_play_log().unwrap();
        sync(&mut db, &files);
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM play_log", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn playlists_by_uuid_the_latest_wins_and_deletions_hold() {
        let mut db = db_with_songs();
        let mine = db.create_playlist("Mine", 1).unwrap();
        db.add_to_playlist(mine, &[id_of(&db, "/m/Adele/25/01.flac")], None, 1)
            .unwrap();
        let song = |path: Option<&str>, uri: Option<&str>, title: &str| {
            json!({ "path": path, "uri": uri,
            "title": title, "artists": ["Adele"], "durationMs": 180_000 })
        };
        let files = [phone(json!({ "playlists": [
            { "id": "p1", "name": "From the Phone", "created": 2_000, "modified": 3_000, "deleted": false,
              "songs": [song(Some("Adele/25/02.flac"), None, "Send My Love"),
                        song(None, Some("spotify:track:s"), "Streamed"),
                        song(Some("Not/Here.flac"), None, "Not Here")] }
        ] }))];
        let (file, _) = sync(&mut db, &files);
        let p1: i64 = db
            .conn
            .query_row("SELECT id FROM playlist WHERE sync_id = 'p1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let items = db.playlist_items(p1).unwrap();
        assert_eq!(
            items
                .iter()
                .map(|i| i.match_state.as_str())
                .collect::<Vec<_>>(),
            ["local", "spotify_only", "unavailable"]
        );
        assert_eq!(
            items[0].track.as_ref().map(|t| t.record.id),
            Some(id_of(&db, "/m/Adele/25/02.flac"))
        );
        // This side's own playlist went out with its UUID and the song's path.
        let out: Vec<&Value> = file["playlists"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["name"] == "Mine")
            .collect();
        assert_eq!(out[0]["songs"][0]["path"], "Adele/25/01.flac");
        assert!(out[0]["id"].as_str().is_some_and(|id| id.len() == 36));

        // Deleted on the phone later: gone here, and the deletion goes out again.
        let files = [phone(json!({ "playlists": [
            { "id": "p1", "name": "", "created": 4_000, "modified": 4_000, "deleted": true, "songs": [] }
        ] }))];
        let (file, _) = sync(&mut db, &files);
        assert!(
            db.conn
                .query_row("SELECT id FROM playlist WHERE sync_id = 'p1'", [], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()
                .unwrap()
                .is_none()
        );
        assert!(
            file["playlists"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["id"] == "p1" && p["deleted"] == true)
        );
        // An older change of it doesn't bring it back.
        let files = [phone(json!({ "playlists": [
            { "id": "p1", "name": "Back?", "created": 1, "modified": 3_500, "deleted": false, "songs": [] }
        ] }))];
        sync(&mut db, &files);
        assert!(
            db.conn
                .query_row("SELECT id FROM playlist WHERE sync_id = 'p1'", [], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn choices_isrcs_and_lyrics_the_latest_wins() {
        let mut db = db_with_songs();
        let list = db.create_playlist("Spotify", 1).unwrap();
        db.conn
            .execute(
                "INSERT INTO playlist_item (playlist_id, position, spotify_uri, title, match_state, choice_at)
                 VALUES (?1, 0, 'spotify:track:a', 'A', 'spotify_only', NULL),
                        (?1, 1, 'spotify:track:b', 'B', 'user_pinned_spotify', 9000)",
                [list],
            )
            .unwrap();
        db.conn
            .execute("INSERT INTO mb_isrc_release VALUES ('r1', 10)", [])
            .unwrap();
        let files = [phone(json!({
            "choices": [
                { "uri": "spotify:track:a", "choice": "file", "at": 100,
                  "file": { "path": "Adele/25/01.flac", "title": "Hello", "durationMs": 180_000 } },
                // Older than this side's choice: stays.
                { "uri": "spotify:track:b", "choice": null, "at": 50 }
            ],
            "isrcs": [{ "release": "r1", "checked": 20, "isrcs": [[1, 1, "gbaye9701274"]] }],
            "lyrics": [{ "uri": "spotify:track:a", "checked": 30, "found": true, "synced": "[00:01.00]Hi",
                         "instrumental": false }]
        }))];
        let (file, _) = sync(&mut db, &files);
        let items = db.playlist_items(list).unwrap();
        assert_eq!(items[0].match_state, "user_matched");
        assert_eq!(
            items[0].track.as_ref().map(|t| t.record.id),
            Some(id_of(&db, "/m/Adele/25/01.flac"))
        );
        assert_eq!(items[1].match_state, "user_pinned_spotify");
        let isrc: String = db
            .conn
            .query_row("SELECT isrc FROM mb_isrc WHERE release = 'r1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(isrc, "GBAYE9701274");
        let synced: String = db
            .conn
            .query_row("SELECT synced FROM online_lyrics", [], |row| row.get(0))
            .unwrap();
        assert_eq!(synced, "[00:01.00]Hi");
        // And they go out again: the file choice with its path.
        let a = file["choices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["uri"] == "spotify:track:a")
            .unwrap();
        assert_eq!(a["file"]["path"], "Adele/25/01.flac");
    }

    #[test]
    fn files_that_arent_sync_files_or_newer_are_skipped() {
        let mut db = Database::open_in_memory().unwrap();
        let files = [
            ("jewelcase-sync-x.json".to_owned(), "{ nope".to_owned()),
            (
                "jewelcase-sync-y.json".to_owned(),
                json!({ "format": FORMAT, "version": 9, "device": "y" }).to_string(),
            ),
        ];
        let (_, result) = sync(&mut db, &files);
        assert_eq!(
            result.skipped,
            ["jewelcase-sync-x.json", "jewelcase-sync-y.json"]
        );
    }

    /// A file Jewelcase Mobile wrote (on its test emulator, with its generated test media), as it is.
    const FROM_THE_PHONE: &str = r#"{"format":"jewelcase-sync","version":1,"device":"1886a561-a974-4e55-b35c-fcb7665daf2a","name":"Jewelcase Mobile","written":1795434236141,"choices":[],"isrcs":[],"lyrics":[],"plays":[],"playlists":[{"id":"desk-list-1","name":"From the Desktop","created":1795300000000,"modified":1795300000000,"deleted":false,"songs":[{"path":"Northern Lights\/Dawn Chorus\/03 Lantern.flac","title":"Lantern","artists":["Northern Lights"],"album":"Dawn Chorus","durationMs":34000},{"path":"Somewhere\/Else.flac","title":"Not on the Phone","artists":["Someone"],"durationMs":200000}]}]}"#;

    #[test]
    fn reads_a_file_jewelcase_mobile_wrote() {
        let mut db = db_with_songs();
        let files = [(
            "jewelcase-sync-phone.json".to_owned(),
            FROM_THE_PHONE.to_owned(),
        )];
        let (file, result) = sync(&mut db, &files);
        assert_eq!(result.synced(), ["Jewelcase Mobile"]);
        let id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM playlist WHERE sync_id = 'desk-list-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(db.playlist(id).unwrap().unwrap().name, "From the Desktop");
        assert_eq!(db.playlist_items(id).unwrap().len(), 2);
        // What this side writes, for Jewelcase Mobile's own test of reading it.
        if std::env::var_os("JEWELCASE_PRINT_SYNC_FILE").is_some() {
            println!("{file}");
        }
    }
}
