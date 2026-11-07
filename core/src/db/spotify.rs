//! Spotify playlists in the database (CONCEPT.md §9.5, §9.6): mirrors,
//! matching their entries to the library, and the outbox of changes.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};

use super::{Database, Result};
use crate::matching::{Confidence, LocalTrack, MatchIndex, RemoteTrack};

/// A Spotify playlist as listed by Spotify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotifyPlaylistInfo {
    pub spotify_id: String,
    pub name: String,
    pub owner: Option<String>,
    pub editable: bool,
    pub readable: bool,
    pub image_url: Option<String>,
}

/// A playlist linked to Spotify, for syncing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedPlaylist {
    pub id: i64,
    pub spotify_id: String,
    pub snapshot_id: Option<String>,
    /// 'spotify' (a mirror) or 'local' (published from here).
    pub source: String,
    pub editable: bool,
    pub readable: bool,
    pub image_url: Option<String>,
    pub cover_key: Option<String>,
}

/// One position of a Spotify playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub uri: Option<String>,
    /// `None` for anything that isn't a track (episodes).
    pub track: Option<RemoteTrack>,
}

/// What is known about a library song on Spotify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotifyVersion {
    /// `None`: searched, nothing found.
    pub uri: Option<String>,
    pub confidence: Option<Confidence>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub checked: i64,
}

/// An entry kept across a re-read: one without a Spotify URI (library
/// only), placed after the Spotify entry it followed.
struct Kept {
    id: i64,
    /// The URI before it and which occurrence of that URI it was.
    anchor: Option<(String, usize)>,
}

impl Database {
    /// Every playlist linked to Spotify.
    pub fn linked_playlists(&self) -> Result<Vec<LinkedPlaylist>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, spotify_id, snapshot_id, source, editable, readable, spotify_image_url,
                    spotify_cover_key
             FROM playlist WHERE spotify_id IS NOT NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(LinkedPlaylist {
                id: row.get(0)?,
                spotify_id: row.get(1)?,
                snapshot_id: row.get(2)?,
                source: row.get(3)?,
                editable: row.get(4)?,
                readable: row.get(5)?,
                image_url: row.get(6)?,
                cover_key: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn linked_playlist(&self, id: i64) -> Result<Option<LinkedPlaylist>> {
        Ok(self.linked_playlists()?.into_iter().find(|p| p.id == id))
    }

    /// Adds a Spotify playlist's mirror, or updates its details. Returns
    /// its ID. The name isn't overwritten while a rename waits to be sent.
    pub fn upsert_spotify_playlist(&mut self, info: &SpotifyPlaylistInfo, now: i64) -> Result<i64> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM playlist WHERE spotify_id = ?1",
                [&info.spotify_id],
                |row| row.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => {
                let renaming: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM spotify_outbox
                       WHERE playlist_id = ?1 AND op LIKE '%\"op\":\"rename\"%')",
                    [id],
                    |row| row.get(0),
                )?;
                self.conn.execute(
                    "UPDATE playlist SET
                       name = CASE WHEN ?6 THEN name ELSE ?2 END,
                       spotify_owner = ?3, editable = ?4, spotify_image_url = ?7,
                       -- Once Spotify refused the entries, only owning or
                       -- collaborating makes them readable again.
                       readable = CASE WHEN ?4 THEN 1 ELSE min(readable, ?5) END
                     WHERE id = ?1",
                    params![
                        id,
                        info.name,
                        info.owner,
                        info.editable,
                        info.readable,
                        renaming,
                        info.image_url
                    ],
                )?;
                id
            }
            None => {
                self.conn.execute(
                    "INSERT INTO playlist (name, source, created, modified, spotify_id,
                       spotify_owner, editable, readable, spotify_image_url)
                     VALUES (?1, 'spotify', ?2, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        info.name,
                        now,
                        info.spotify_id,
                        info.owner,
                        info.editable,
                        info.readable,
                        info.image_url
                    ],
                )?;
                self.conn.last_insert_rowid()
            }
        };
        let name: String =
            self.conn
                .query_row("SELECT name FROM playlist WHERE id = ?1", [id], |row| {
                    row.get(0)
                })?;
        self.index_playlist(id, &name)?;
        Ok(id)
    }

    /// After listing the user's Spotify playlists: mirrors of playlists
    /// that are gone are deleted, published local playlists unlinked.
    pub fn forget_spotify_playlists(&mut self, keep: &[String]) -> Result<()> {
        let linked = self.linked_playlists()?;
        for playlist in linked {
            if keep.contains(&playlist.spotify_id) {
                continue;
            }
            if playlist.source == "spotify" {
                self.delete_playlist(playlist.id)?;
            } else {
                self.unlink_playlist(playlist.id)?;
            }
        }
        Ok(())
    }

    /// Ties a local playlist to the Spotify playlist made from it.
    pub fn link_playlist(
        &mut self,
        id: i64,
        spotify_id: &str,
        snapshot_id: &str,
        owner: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist SET spotify_id = ?2, snapshot_id = ?3, spotify_owner = ?4,
               editable = 1, readable = 1 WHERE id = ?1",
            params![id, spotify_id, snapshot_id, owner],
        )?;
        Ok(())
    }

    /// The local playlist stays; the Spotify one is no longer synced.
    pub fn unlink_playlist(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE playlist SET spotify_id = NULL, snapshot_id = NULL, spotify_owner = NULL,
               source = 'local', editable = 1, readable = 1, spotify_image_url = NULL
             WHERE id = ?1",
            [id],
        )?;
        tx.execute("DELETE FROM spotify_outbox WHERE playlist_id = ?1", [id])?;
        // Entries that only exist on Spotify can't play any more.
        tx.execute(
            "UPDATE playlist_item SET
               match_state = CASE WHEN local_track_id IS NULL THEN 'unavailable' ELSE 'local' END,
               suggested_track_id = NULL, suggested_spotify = NULL
             WHERE playlist_id = ?1",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Disconnecting: mirrors go, published playlists stay local.
    pub fn forget_spotify(&mut self) -> Result<()> {
        self.forget_spotify_playlists(&[])?;
        self.conn.execute("DELETE FROM spotify_track", [])?;
        Ok(())
    }

    /// Spotify refused the entries (403): not asked again for this
    /// snapshot, and shown as hidden.
    pub fn mark_unreadable(&mut self, id: i64, snapshot_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist SET readable = 0, snapshot_id = ?2 WHERE id = ?1",
            params![id, snapshot_id],
        )?;
        Ok(())
    }

    pub fn set_snapshot(&mut self, id: i64, snapshot_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist SET snapshot_id = ?2 WHERE id = ?1",
            params![id, snapshot_id],
        )?;
        Ok(())
    }

    pub fn set_spotify_cover(&mut self, id: i64, key: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist SET spotify_cover_key = ?2 WHERE id = ?1",
            params![id, key],
        )?;
        Ok(())
    }

    /// The library, for matching.
    pub fn match_tracks(&self) -> Result<Vec<LocalTrack>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.title, t.artist, t.album_artist, COALESCE(a.title, t.album),
                    t.duration_ms, t.isrc, t.format
             FROM track t LEFT JOIN album a ON a.id = t.album_id
             WHERE t.title IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(LocalTrack {
                track_id: row.get(0)?,
                title: row.get(1)?,
                artist: row.get(2)?,
                album_artist: row.get(3)?,
                album: row.get(4)?,
                duration_ms: row.get::<_, i64>(5)?.max(0) as u64,
                isrc: row.get(6)?,
                format: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The Spotify URIs of a playlist in order: what the Spotify side
    /// should be.
    pub fn spotify_uris(&self, id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT spotify_uri FROM playlist_item
             WHERE playlist_id = ?1 AND spotify_uri IS NOT NULL ORDER BY position",
        )?;
        let rows = stmt.query_map([id], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// An entry's position among the Spotify entries (where Spotify has it,
    /// or would put it); `None` if the entry is gone.
    pub fn remote_position(&self, item_id: i64) -> Result<Option<u32>> {
        Ok(self
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM playlist_item o
                          WHERE o.playlist_id = i.playlist_id AND o.spotify_uri IS NOT NULL
                            AND o.position < i.position)
                 FROM playlist_item i WHERE i.id = ?1",
                [item_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// The entry at a position of a playlist.
    pub fn item_at(
        &self,
        playlist_id: i64,
        position: u32,
    ) -> Result<Option<(i64, Option<String>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, spotify_uri FROM playlist_item WHERE playlist_id = ?1 AND position = ?2",
                params![playlist_id, position],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub fn item_uri(&self, item_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT spotify_uri FROM playlist_item WHERE id = ?1",
                [item_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    /// Replaces a playlist's Spotify entries with what Spotify has, matched
    /// to the library. The user's choices (by URI) and the entries that
    /// only exist here are kept.
    pub fn set_spotify_items(
        &mut self,
        id: i64,
        entries: &[RemoteEntry],
        index: &MatchIndex,
        snapshot_id: &str,
        now: i64,
    ) -> Result<()> {
        // What to keep from before.
        let mut choices: HashMap<String, (String, Option<i64>)> = HashMap::new();
        let mut kept = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, spotify_uri, match_state, local_track_id FROM playlist_item
                 WHERE playlist_id = ?1 ORDER BY position",
            )?;
            let rows: Vec<(i64, Option<String>, String, Option<i64>)> = stmt
                .query_map([id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let mut anchor: Option<(String, usize)> = None;
            let mut seen: HashMap<String, usize> = HashMap::new();
            for (item, uri, state, track) in rows {
                match uri {
                    Some(uri) => {
                        if state == "user_matched" || state == "user_pinned_spotify" {
                            choices.insert(uri.clone(), (state, track));
                        }
                        let count = seen.entry(uri.clone()).or_default();
                        anchor = Some((uri, *count));
                        *count += 1;
                    }
                    None => kept.push(Kept {
                        id: item,
                        anchor: anchor.clone(),
                    }),
                }
            }
        }

        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM playlist_item WHERE playlist_id = ?1 AND spotify_uri IS NOT NULL",
            [id],
        )?;
        // New entries, then everything in order.
        let mut order: Vec<i64> = kept
            .iter()
            .filter(|k| k.anchor.is_none())
            .map(|k| k.id)
            .collect();
        let mut seen: HashMap<&str, usize> = HashMap::new();
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO playlist_item (playlist_id, position, local_track_id, spotify_uri,
                   isrc, title, artist, album, duration_ms, match_state, suggested_track_id)
                 VALUES (?1, -1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            let exists = |track: i64| -> rusqlite::Result<bool> {
                tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM track WHERE id = ?1)",
                    [track],
                    |row| row.get(0),
                )
            };
            for entry in entries {
                let Some(uri) = entry.uri.as_deref() else {
                    continue;
                };
                let (state, local, suggested) = match (&entry.track, choices.get(uri)) {
                    (_, Some((state, Some(track))))
                        if state == "user_matched" && exists(*track)? =>
                    {
                        ("user_matched", Some(*track), None)
                    }
                    (_, Some((state, _))) if state == "user_pinned_spotify" => {
                        ("user_pinned_spotify", None, None)
                    }
                    (Some(track), _) => match index.find(track) {
                        Some((local, Confidence::Sure)) => ("matched", Some(local), None),
                        Some((local, Confidence::Likely)) => ("spotify_only", None, Some(local)),
                        None => ("spotify_only", None, None),
                    },
                    (None, _) => ("unavailable", None, None),
                };
                let track = entry.track.as_ref();
                insert.execute(params![
                    id,
                    local,
                    uri,
                    track.and_then(|t| t.isrc.clone()),
                    track.map(|t| t.title.clone()),
                    track.map(|t| t.artists.join(", ")),
                    track.and_then(|t| t.album.clone()),
                    track.map(|t| t.duration_ms as i64),
                    state,
                    suggested,
                ])?;
                order.push(tx.last_insert_rowid());
                let count = seen.entry(uri).or_default();
                let anchor = (uri.to_owned(), *count);
                *count += 1;
                order.extend(
                    kept.iter()
                        .filter(|k| k.anchor.as_ref() == Some(&anchor))
                        .map(|k| k.id),
                );
            }
        }
        // Entries whose anchor is gone go to the end.
        for k in &kept {
            if !order.contains(&k.id) {
                order.push(k.id);
            }
        }
        {
            let mut update =
                tx.prepare_cached("UPDATE playlist_item SET position = ?2 WHERE id = ?1")?;
            for (position, item) in order.iter().enumerate() {
                update.execute(params![item, position as i64])?;
            }
        }
        tx.execute(
            "UPDATE playlist SET snapshot_id = ?2, modified = ?3 WHERE id = ?1",
            params![id, snapshot_id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// After library changes: Spotify entries are matched again, so a
    /// newly imported CD takes over its entries (§9.5). The user's choices
    /// stay. Returns how many entries changed.
    pub fn rematch_spotify_items(&mut self, index: &MatchIndex) -> Result<usize> {
        let rows: Vec<(i64, String, Option<i64>, RemoteTrack)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, match_state, local_track_id, title, artist, album, duration_ms, isrc
                 FROM playlist_item
                 WHERE spotify_uri IS NOT NULL AND title IS NOT NULL
                   AND (match_state IN ('matched', 'spotify_only')
                        OR (match_state = 'user_matched' AND local_track_id IS NULL))",
            )?;
            stmt.query_map([], |row| {
                let artist: Option<String> = row.get(4)?;
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    RemoteTrack {
                        title: row.get(3)?,
                        artists: artist
                            .map(|a| a.split(", ").map(str::to_owned).collect())
                            .unwrap_or_default(),
                        album: row.get(5)?,
                        duration_ms: row.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0) as u64,
                        isrc: row.get(7)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        let mut changed = 0;
        let tx = self.conn.transaction()?;
        for (item, state, local, remote) in rows {
            let (new_state, new_local, suggested) = match index.find(&remote) {
                Some((track, Confidence::Sure)) => ("matched", Some(track), None),
                Some((track, Confidence::Likely)) => ("spotify_only", None, Some(track)),
                None => ("spotify_only", None, None),
            };
            if new_state != state || new_local != local {
                changed += 1;
            }
            tx.execute(
                "UPDATE playlist_item SET match_state = ?2, local_track_id = ?3,
                   suggested_track_id = ?4 WHERE id = ?1",
                params![item, new_state, new_local, suggested],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// The user picked the library song for a Spotify entry (`None`: let
    /// matching decide again).
    pub fn choose_local_match(&mut self, item_id: i64, track_id: Option<i64>) -> Result<()> {
        match track_id {
            Some(track) => self.conn.execute(
                "UPDATE playlist_item SET local_track_id = ?2, match_state = 'user_matched',
                   suggested_track_id = NULL WHERE id = ?1",
                params![item_id, track],
            )?,
            None => self.conn.execute(
                "UPDATE playlist_item SET local_track_id = NULL, match_state = 'spotify_only',
                   suggested_track_id = NULL WHERE id = ?1",
                [item_id],
            )?,
        };
        Ok(())
    }

    /// "Not this song": drops a suggestion without choosing.
    pub fn dismiss_suggestion(&mut self, item_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist_item SET suggested_track_id = NULL, suggested_spotify = NULL
             WHERE id = ?1",
            [item_id],
        )?;
        Ok(())
    }

    /// "Always play from Spotify" (§9.5).
    pub fn pin_spotify(&mut self, item_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist_item SET local_track_id = NULL, match_state = 'user_pinned_spotify',
               suggested_track_id = NULL WHERE id = ?1 AND spotify_uri IS NOT NULL",
            [item_id],
        )?;
        Ok(())
    }

    /// A library entry now has its Spotify version.
    pub fn set_item_spotify(&mut self, item_id: i64, uri: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist_item SET spotify_uri = ?2, suggested_spotify = NULL,
               match_state = CASE WHEN local_track_id IS NULL THEN 'spotify_only' ELSE 'matched' END
             WHERE id = ?1",
            params![item_id, uri],
        )?;
        Ok(())
    }

    /// A Spotify track that is probably this library entry (JSON), or none.
    pub fn set_item_suggestion(&mut self, item_id: i64, suggestion: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE playlist_item SET suggested_spotify = ?2 WHERE id = ?1",
            params![item_id, suggestion],
        )?;
        Ok(())
    }

    /// Library entries of a linked playlist without a Spotify version.
    pub fn items_without_uri(&self, playlist_id: i64) -> Result<Vec<(i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, local_track_id FROM playlist_item
             WHERE playlist_id = ?1 AND spotify_uri IS NULL AND local_track_id IS NOT NULL
             ORDER BY position",
        )?;
        let rows = stmt.query_map([playlist_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // The Spotify versions of library songs ---------------------------------

    pub fn spotify_version(&self, track_id: i64) -> Result<Option<SpotifyVersion>> {
        Ok(self
            .conn
            .query_row(
                "SELECT uri, confidence, title, artist, checked FROM spotify_track
                 WHERE track_id = ?1",
                [track_id],
                |row| {
                    let confidence: Option<String> = row.get(1)?;
                    Ok(SpotifyVersion {
                        uri: row.get(0)?,
                        confidence: confidence.map(|c| {
                            if c == "sure" {
                                Confidence::Sure
                            } else {
                                Confidence::Likely
                            }
                        }),
                        title: row.get(2)?,
                        artist: row.get(3)?,
                        checked: row.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_spotify_version(&mut self, track_id: i64, version: &SpotifyVersion) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO spotify_track (track_id, uri, confidence, title, artist, checked)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                track_id,
                version.uri,
                version.confidence.map(|c| match c {
                    Confidence::Sure => "sure",
                    Confidence::Likely => "likely",
                }),
                version.title,
                version.artist,
                version.checked
            ],
        )?;
        Ok(())
    }

    /// A URI some playlist already ties to this library song.
    pub fn known_uri(&self, track_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT spotify_uri FROM playlist_item
                 WHERE local_track_id = ?1 AND spotify_uri LIKE 'spotify:track:%'
                   AND match_state IN ('matched', 'user_matched')
                 LIMIT 1",
                [track_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    // Outbox (§9.6) ------------------------------------------------------------

    pub fn queue_spotify_op(&mut self, playlist_id: i64, op: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO spotify_outbox (playlist_id, op, created) VALUES (?1, ?2, ?3)",
            params![playlist_id, op, now],
        )?;
        Ok(())
    }

    /// Waiting changes of a playlist, oldest first.
    pub fn spotify_ops(&self, playlist_id: i64) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, op FROM spotify_outbox WHERE playlist_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([playlist_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn playlists_with_ops(&self) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT playlist_id FROM spotify_outbox ORDER BY playlist_id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_spotify_op(&mut self, op_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM spotify_outbox WHERE id = ?1", [op_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{file, rebuild};
    use super::*;
    use crate::library::artists::Aliases;

    fn remote(uri: &str, title: &str, duration_s: u64) -> RemoteEntry {
        RemoteEntry {
            uri: Some(uri.into()),
            track: Some(RemoteTrack {
                title: title.into(),
                artists: vec!["Adele".into()],
                album: None,
                duration_ms: duration_s * 1000,
                isrc: None,
            }),
        }
    }

    fn info(id: &str) -> SpotifyPlaylistInfo {
        SpotifyPlaylistInfo {
            spotify_id: id.into(),
            name: "On Spotify".into(),
            owner: Some("Max".into()),
            editable: true,
            readable: true,
            image_url: None,
        }
    }

    fn states(db: &Database, id: i64) -> Vec<(Option<String>, String, bool)> {
        db.playlist_items(id)
            .unwrap()
            .into_iter()
            .map(|i| (i.spotify_uri, i.match_state, i.track.is_some()))
            .collect()
    }

    #[test]
    fn spotify_entries_are_matched_and_choices_survive_a_reread() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(
            &[
                file("/m/25/1.flac", "25", "Hello", 1),
                file("/m/25/2.flac", "25", "Send My Love", 2),
            ],
            1,
        )
        .unwrap();
        rebuild(&mut db);
        let aliases = Aliases::default();
        let index = MatchIndex::new(db.match_tracks().unwrap(), &aliases);

        let id = db.upsert_spotify_playlist(&info("sp1"), 5).unwrap();
        assert_eq!(db.upsert_spotify_playlist(&info("sp1"), 6).unwrap(), id);
        let entries = vec![
            remote("spotify:track:hello", "Hello", 181),
            remote("spotify:track:other", "Skyfall", 286),
            remote(
                "spotify:track:love",
                "Send My Love (To Your New Lover)",
                186,
            ),
            RemoteEntry {
                uri: Some("spotify:episode:e".into()),
                track: None,
            },
        ];
        db.set_spotify_items(id, &entries, &index, "snap1", 7)
            .unwrap();
        let playlist = db.playlist(id).unwrap().unwrap();
        assert_eq!(playlist.source, "spotify");
        assert_eq!((playlist.item_count, playlist.local_count), (4, 1));
        assert_eq!(
            states(&db, id),
            [
                (Some("spotify:track:hello".into()), "matched".into(), true),
                (
                    Some("spotify:track:other".into()),
                    "spotify_only".into(),
                    false
                ),
                // 6 s longer: only a suggestion.
                (
                    Some("spotify:track:love".into()),
                    "spotify_only".into(),
                    false
                ),
                (
                    Some("spotify:episode:e".into()),
                    "unavailable".into(),
                    false
                ),
            ]
        );

        let items = db.playlist_items(id).unwrap();
        assert_eq!(
            items[2]
                .suggested_track
                .as_ref()
                .and_then(|t| t.record.tags.title.as_deref()),
            Some("Send My Love")
        );
        // The user confirms it and pins another to Spotify; a re-read
        // keeps both.
        let love = items[2].suggested_track.as_ref().unwrap().record.id;
        db.choose_local_match(items[2].id, Some(love)).unwrap();
        db.pin_spotify(items[0].id).unwrap();
        db.set_spotify_items(id, &entries, &index, "snap2", 8)
            .unwrap();
        assert_eq!(states(&db, id)[0].1, "user_pinned_spotify");
        assert_eq!(
            states(&db, id)[2],
            (
                Some("spotify:track:love".into()),
                "user_matched".into(),
                true
            )
        );
        assert_eq!(
            db.linked_playlists().unwrap()[0].snapshot_id.as_deref(),
            Some("snap2")
        );
        assert_eq!(db.spotify_uris(id).unwrap().len(), 4);
    }

    #[test]
    fn library_only_entries_keep_their_place() {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_tracks(&[file("/m/25/1.flac", "25", "Hello", 1)], 1)
            .unwrap();
        rebuild(&mut db);
        let track = db.all_tracks().unwrap()[0].id;
        let aliases = Aliases::default();
        let index = MatchIndex::new(Vec::new(), &aliases);

        // A local playlist published to Spotify: a, [Hello], b.
        let id = db.create_playlist("Mixed", 1).unwrap();
        db.link_playlist(id, "sp2", "s0", None).unwrap();
        let a = remote("spotify:track:a", "A", 100);
        let b = remote("spotify:track:b", "B", 100);
        db.set_spotify_items(id, &[a.clone(), b.clone()], &index, "s1", 2)
            .unwrap();
        db.add_to_playlist(id, &[track], Some(1), 3).unwrap();
        assert_eq!(
            db.remote_position(db.playlist_items(id).unwrap()[2].id)
                .unwrap(),
            Some(1)
        );

        // Someone added c at the front on their phone.
        let c = remote("spotify:track:c", "C", 100);
        db.set_spotify_items(id, &[c, a, b], &index, "s2", 4)
            .unwrap();
        let order: Vec<Option<String>> = db
            .playlist_items(id)
            .unwrap()
            .into_iter()
            .map(|i| i.spotify_uri.or(i.title))
            .collect();
        assert_eq!(
            order,
            [
                Some("spotify:track:c".into()),
                Some("spotify:track:a".into()),
                Some("Hello".into()),
                Some("spotify:track:b".into()),
            ]
        );
        assert_eq!(db.items_without_uri(id).unwrap().len(), 1);

        // Unlinking keeps the library song, the Spotify-only ones can't play.
        db.unlink_playlist(id).unwrap();
        let playlist = db.playlist(id).unwrap().unwrap();
        assert_eq!(playlist.spotify_id, None);
        assert_eq!(states(&db, id)[2].1, "local");
        assert_eq!(states(&db, id)[0].1, "unavailable");
    }

    #[test]
    fn a_new_album_takes_over_spotify_entries() {
        let mut db = Database::open_in_memory().unwrap();
        let aliases = Aliases::default();
        let id = db.upsert_spotify_playlist(&info("sp3"), 1).unwrap();
        let empty = MatchIndex::new(Vec::new(), &aliases);
        db.set_spotify_items(
            id,
            &[remote("spotify:track:h", "Hello", 180)],
            &empty,
            "s",
            2,
        )
        .unwrap();
        assert_eq!(states(&db, id)[0].1, "spotify_only");

        db.upsert_tracks(&[file("/m/25/1.flac", "25", "Hello", 1)], 3)
            .unwrap();
        rebuild(&mut db);
        let index = MatchIndex::new(db.match_tracks().unwrap(), &aliases);
        assert_eq!(db.rematch_spotify_items(&index).unwrap(), 1);
        assert_eq!(
            states(&db, id)[0],
            (Some("spotify:track:h".into()), "matched".into(), true)
        );
        // Deleting the playlist on Spotify deletes the mirror.
        db.forget_spotify_playlists(&[]).unwrap();
        assert!(db.playlist(id).unwrap().is_none());
    }

    #[test]
    fn refused_playlists_stay_hidden() {
        let mut db = Database::open_in_memory().unwrap();
        let mut theirs = info("sp5");
        theirs.editable = false;
        let id = db.upsert_spotify_playlist(&theirs, 1).unwrap();
        assert!(db.playlist(id).unwrap().unwrap().readable);
        db.mark_unreadable(id, "s1").unwrap();
        db.upsert_spotify_playlist(&theirs, 2).unwrap();
        assert!(!db.playlist(id).unwrap().unwrap().readable);
        // Made collaborative: readable again.
        theirs.editable = true;
        db.upsert_spotify_playlist(&theirs, 3).unwrap();
        assert!(db.playlist(id).unwrap().unwrap().readable);
    }

    #[test]
    fn the_outbox_keeps_order() {
        let mut db = Database::open_in_memory().unwrap();
        let id = db.upsert_spotify_playlist(&info("sp4"), 1).unwrap();
        db.queue_spotify_op(id, r#"{"op":"rename","name":"New"}"#, 2)
            .unwrap();
        db.queue_spotify_op(id, r#"{"op":"move"}"#, 3).unwrap();
        // A waiting rename keeps the new name.
        db.rename_playlist(id, "New", 2).unwrap();
        db.upsert_spotify_playlist(&info("sp4"), 4).unwrap();
        let playlist = db.playlist(id).unwrap().unwrap();
        assert_eq!(
            (playlist.name.as_str(), playlist.pending_changes),
            ("New", 2)
        );
        let ops = db.spotify_ops(id).unwrap();
        assert!(ops[0].1.contains("rename"));
        db.remove_spotify_op(ops[0].0).unwrap();
        assert_eq!(db.playlists_with_ops().unwrap(), [id]);
    }
}
