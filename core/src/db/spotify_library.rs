//! Saved albums, followed artists and discographies from Spotify
//! (CONCEPT.md §9.8): shown apart from the library, never mixed into it.

use rusqlite::{OptionalExtension, Row, params};

use super::{Database, Result};

/// An album on Spotify.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpotifyAlbum {
    pub spotify_id: String,
    pub name: String,
    /// "Artist, Artist".
    pub artist: String,
    pub artist_ids: Vec<String>,
    /// "album", "single" (singles and EPs) or "compilation".
    pub album_type: String,
    pub year: Option<i32>,
    pub total_tracks: u32,
    pub image_url: Option<String>,
    pub upc: Option<String>,
}

impl SpotifyAlbum {
    /// Its cover in the cover cache, once downloaded.
    pub fn cover_key(&self) -> Option<String> {
        self.image_url
            .as_deref()
            .map(|url| crate::library::covers::cover_key('s', url.as_bytes()))
    }

    pub fn is_single(&self) -> bool {
        self.album_type == "single"
    }
}

/// An artist on Spotify.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpotifyArtist {
    pub spotify_id: String,
    pub name: String,
    pub image_url: Option<String>,
    pub genres: Vec<String>,
}

impl SpotifyArtist {
    pub fn cover_key(&self) -> Option<String> {
        self.image_url
            .as_deref()
            .map(|url| crate::library::covers::cover_key('s', url.as_bytes()))
    }
}

const ALBUM_COLUMNS: &str = "a.spotify_id, a.name, a.artist, a.artist_ids, a.album_type, a.year, a.total_tracks, a.image_url, a.upc";

fn album_row(row: &Row) -> rusqlite::Result<SpotifyAlbum> {
    let ids: Option<String> = row.get(3)?;
    Ok(SpotifyAlbum {
        spotify_id: row.get(0)?,
        name: row.get(1)?,
        artist: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        artist_ids: ids
            .map(|ids| {
                ids.split(',')
                    .filter(|i| !i.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        album_type: row.get(4)?,
        year: row.get(5)?,
        total_tracks: row.get::<_, i64>(6)?.max(0) as u32,
        image_url: row.get(7)?,
        upc: row.get(8)?,
    })
}

fn artist_row(row: &Row) -> rusqlite::Result<SpotifyArtist> {
    let genres: Option<String> = row.get(3)?;
    Ok(SpotifyArtist {
        spotify_id: row.get(0)?,
        name: row.get(1)?,
        image_url: row.get(2)?,
        genres: genres
            .map(|g| {
                g.split(',')
                    .filter(|g| !g.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn upsert_album(conn: &rusqlite::Connection, album: &SpotifyAlbum) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO spotify_album (spotify_id, name, artist, artist_ids, album_type, year,
           total_tracks, image_url, upc)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(spotify_id) DO UPDATE SET name = excluded.name, artist = excluded.artist,
           artist_ids = excluded.artist_ids, album_type = excluded.album_type,
           year = excluded.year, total_tracks = excluded.total_tracks,
           image_url = COALESCE(excluded.image_url, image_url),
           upc = COALESCE(excluded.upc, upc)",
        params![
            album.spotify_id,
            album.name,
            album.artist,
            album.artist_ids.join(","),
            album.album_type,
            album.year,
            i64::from(album.total_tracks),
            album.image_url,
            album.upc,
        ],
    )?;
    Ok(())
}

impl Database {
    /// The saved albums as Spotify lists them now (the latest first).
    pub fn replace_saved_albums(&mut self, albums: &[SpotifyAlbum]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE spotify_album SET saved_position = NULL", [])?;
        for (position, album) in albums.iter().enumerate() {
            upsert_album(&tx, album)?;
            tx.execute(
                "UPDATE spotify_album SET saved_position = ?2 WHERE spotify_id = ?1",
                params![album.spotify_id, position as i64],
            )?;
        }
        // Albums nothing shows any more.
        tx.execute(
            "DELETE FROM spotify_album WHERE saved_position IS NULL
               AND spotify_id NOT IN (SELECT album_id FROM spotify_discography)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn saved_albums(&self) -> Result<Vec<SpotifyAlbum>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {ALBUM_COLUMNS} FROM spotify_album a
             WHERE a.saved_position IS NOT NULL ORDER BY a.saved_position"
        ))?;
        stmt.query_map([], album_row)?
            .collect::<rusqlite::Result<_>>()
            .map_err(Into::into)
    }

    pub fn spotify_album(&self, spotify_id: &str) -> Result<Option<SpotifyAlbum>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ALBUM_COLUMNS} FROM spotify_album a WHERE a.spotify_id = ?1"),
                [spotify_id],
                album_row,
            )
            .optional()?)
    }

    /// The followed artists as Spotify lists them now.
    pub fn replace_followed_artists(&mut self, artists: &[SpotifyArtist]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE spotify_artist SET followed_position = NULL", [])?;
        for (position, artist) in artists.iter().enumerate() {
            tx.execute(
                "INSERT INTO spotify_artist (spotify_id, name, image_url, genres, followed_position)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(spotify_id) DO UPDATE SET name = excluded.name,
                   image_url = COALESCE(excluded.image_url, image_url),
                   genres = excluded.genres, followed_position = excluded.followed_position",
                params![
                    artist.spotify_id,
                    artist.name,
                    artist.image_url,
                    artist.genres.join(","),
                    position as i64,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn followed_artists(&self) -> Result<Vec<SpotifyArtist>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT spotify_id, name, image_url, genres FROM spotify_artist
             WHERE followed_position IS NOT NULL ORDER BY followed_position",
        )?;
        stmt.query_map([], artist_row)?
            .collect::<rusqlite::Result<_>>()
            .map_err(Into::into)
    }

    /// An artist met on Spotify (followed, or looked up for a page).
    pub fn store_spotify_artist(&mut self, artist: &SpotifyArtist) -> Result<()> {
        self.conn.execute(
            "INSERT INTO spotify_artist (spotify_id, name, image_url, genres)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(spotify_id) DO UPDATE SET name = excluded.name,
               image_url = COALESCE(excluded.image_url, image_url), genres = excluded.genres",
            params![
                artist.spotify_id,
                artist.name,
                artist.image_url,
                artist.genres.join(",")
            ],
        )?;
        Ok(())
    }

    pub fn spotify_artist(&self, spotify_id: &str) -> Result<Option<SpotifyArtist>> {
        Ok(self
            .conn
            .query_row(
                "SELECT spotify_id, name, image_url, genres FROM spotify_artist
                 WHERE spotify_id = ?1",
                [spotify_id],
                artist_row,
            )
            .optional()?)
    }

    /// An artist's releases as Spotify lists them, read at `now`.
    pub fn store_discography(
        &mut self,
        artist_id: &str,
        albums: &[SpotifyAlbum],
        now: i64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM spotify_discography WHERE artist_id = ?1",
            [artist_id],
        )?;
        for (position, album) in albums.iter().enumerate() {
            upsert_album(&tx, album)?;
            tx.execute(
                "INSERT OR IGNORE INTO spotify_discography (artist_id, album_id, position)
                 VALUES (?1, ?2, ?3)",
                params![artist_id, album.spotify_id, position as i64],
            )?;
        }
        tx.execute(
            "INSERT INTO spotify_discography_read (artist_id, read) VALUES (?1, ?2)
             ON CONFLICT(artist_id) DO UPDATE SET read = excluded.read",
            params![artist_id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// An artist's releases on Spotify and when they were read; `None`
    /// if never.
    pub fn discography(&self, artist_id: &str) -> Result<Option<(Vec<SpotifyAlbum>, i64)>> {
        let read: Option<i64> = self
            .conn
            .query_row(
                "SELECT read FROM spotify_discography_read WHERE artist_id = ?1",
                [artist_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(read) = read else { return Ok(None) };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {ALBUM_COLUMNS} FROM spotify_discography d
             JOIN spotify_album a ON a.spotify_id = d.album_id
             WHERE d.artist_id = ?1 ORDER BY d.position"
        ))?;
        let albums = stmt
            .query_map([artist_id], album_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some((albums, read)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, name: &str) -> SpotifyAlbum {
        SpotifyAlbum {
            spotify_id: id.into(),
            name: name.into(),
            artist: "Stray Kids".into(),
            artist_ids: vec!["skz".into()],
            album_type: "album".into(),
            year: Some(2023),
            total_tracks: 8,
            image_url: Some(format!("https://i/{id}")),
            upc: None,
        }
    }

    #[test]
    fn saved_albums_follow_spotify_and_discographies_stay() {
        let mut db = Database::open_in_memory().unwrap();
        db.replace_saved_albums(&[album("a", "ROCK-STAR"), album("b", "5-STAR")])
            .unwrap();
        let names: Vec<String> = db
            .saved_albums()
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["ROCK-STAR", "5-STAR"]);
        assert_eq!(db.spotify_album("a").unwrap().unwrap().artist_ids, ["skz"]);

        // "b" is also in a discography: unsaving keeps it there.
        assert_eq!(db.discography("skz").unwrap(), None);
        db.store_discography("skz", &[album("b", "5-STAR"), album("c", "ODDINARY")], 7)
            .unwrap();
        db.replace_saved_albums(&[album("a", "ROCK-STAR")]).unwrap();
        assert_eq!(db.saved_albums().unwrap().len(), 1);
        let (albums, read) = db.discography("skz").unwrap().unwrap();
        assert_eq!(read, 7);
        assert_eq!(
            albums.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["5-STAR", "ODDINARY"]
        );
    }

    #[test]
    fn followed_artists_follow_spotify() {
        let mut db = Database::open_in_memory().unwrap();
        let artist = |id: &str| SpotifyArtist {
            spotify_id: id.into(),
            name: id.to_uppercase(),
            image_url: None,
            genres: vec!["k-pop".into()],
        };
        db.replace_followed_artists(&[artist("a"), artist("b")])
            .unwrap();
        db.replace_followed_artists(&[artist("b")]).unwrap();
        let followed = db.followed_artists().unwrap();
        assert_eq!(followed.len(), 1);
        assert_eq!(followed[0].genres, ["k-pop"]);
        // Still known (for its page), just not followed.
        assert!(db.spotify_artist("a").unwrap().is_some());
    }
}
