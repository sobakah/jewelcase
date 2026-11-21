//! An artist's most popular songs on Deezer, for their page's Popular
//! section (CONCEPT.md §4.2): kept a day, so pages opened again don't ask
//! Deezer, and kept on when Deezer can't be asked.

use rusqlite::{OptionalExtension, params};

use super::{Database, Result};

/// A song of an artist's Deezer top list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PopularSong {
    /// Deezer's song ID (for its ISRC, when it's played).
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: u64,
    pub cover_url: Option<String>,
}

impl Database {
    /// The Deezer artist `artist_id`'s top list as kept, and when it was
    /// read; `None` if never.
    pub fn popular_songs(&self, artist_id: i64) -> Result<Option<(Vec<PopularSong>, i64)>> {
        let read: Option<i64> = self
            .conn
            .query_row(
                "SELECT read FROM deezer_popular_read WHERE artist_id = ?1",
                [artist_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(read) = read else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT song_id, title, artist, album, duration_ms, cover_url FROM deezer_popular
             WHERE artist_id = ?1 ORDER BY position",
        )?;
        let songs = stmt
            .query_map([artist_id], |row| {
                Ok(PopularSong {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    artist: row.get(2)?,
                    album: row.get(3)?,
                    duration_ms: row.get::<_, i64>(4)? as u64,
                    cover_url: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some((songs, read)))
    }

    /// Keeps an artist's top list, read at `now`, in place of the one
    /// before.
    pub fn store_popular_songs(
        &mut self,
        artist_id: i64,
        songs: &[PopularSong],
        now: i64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM deezer_popular WHERE artist_id = ?1",
            [artist_id],
        )?;
        for (position, song) in songs.iter().enumerate() {
            tx.execute(
                "INSERT INTO deezer_popular
                   (artist_id, position, song_id, title, artist, album, duration_ms, cover_url)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    artist_id,
                    position as i64,
                    song.id,
                    song.title,
                    song.artist,
                    song.album,
                    song.duration_ms as i64,
                    song.cover_url,
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO deezer_popular_read (artist_id, read) VALUES (?1, ?2)
             ON CONFLICT(artist_id) DO UPDATE SET read = excluded.read",
            params![artist_id, now],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A top list is kept in its order, replaced by the next one.
    #[test]
    fn an_artists_top_list_is_kept() {
        let mut db = Database::open_in_memory().unwrap();
        assert_eq!(db.popular_songs(7).unwrap(), None);
        let song = |id: i64, title: &str| PopularSong {
            id,
            title: title.into(),
            artist: "Adele".into(),
            duration_ms: 1000,
            ..PopularSong::default()
        };
        db.store_popular_songs(7, &[song(2, "Skyfall"), song(1, "Hello")], 100)
            .unwrap();
        let (songs, read) = db.popular_songs(7).unwrap().unwrap();
        assert_eq!(read, 100);
        assert_eq!(
            songs.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
            ["Skyfall", "Hello"]
        );
        db.store_popular_songs(7, &[song(1, "Hello")], 200).unwrap();
        assert_eq!(db.popular_songs(7).unwrap().unwrap().0.len(), 1);
    }
}
