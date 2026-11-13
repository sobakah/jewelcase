//! New releases of followed Spotify artists, read from Deezer (CONCEPT.md
//! §9.8), and each followed artist's Deezer artist.

use rusqlite::{OptionalExtension, params};

use super::{Database, Result};

/// A recent release of a followed artist.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewRelease {
    pub deezer_id: i64,
    /// Read: the first of its followed artists (stored: the artist whose
    /// list it's in, see `replace_new_releases`).
    pub artist_spotify_id: String,
    /// Read: the names of its followed artists.
    pub artist: String,
    pub title: String,
    /// "album", "ep" or "single".
    pub kind: String,
    /// YYYY-MM-DD.
    pub release_date: String,
    pub image_url: Option<String>,
    pub upc: Option<String>,
    /// The same release on Spotify, once found there.
    pub spotify_album_id: Option<String>,
    /// Shown in the feed already.
    pub seen: bool,
}

impl NewRelease {
    /// Its cover in the cover cache, once downloaded.
    pub fn cover_key(&self) -> Option<String> {
        self.image_url
            .as_deref()
            .map(|url| crate::library::covers::cover_key('s', url.as_bytes()))
    }
}

/// What Spotify told about a followed artist's albums: to find them on
/// Deezer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownAlbums {
    pub upcs: Vec<String>,
    pub titles: Vec<String>,
}

impl Database {
    /// The artist's Deezer ID (`None` inside: not found there) and when it
    /// was looked for; `None` if never.
    pub fn deezer_artist(&self, spotify_id: &str) -> Result<Option<(Option<i64>, i64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT deezer_id, matched FROM deezer_artist WHERE spotify_id = ?1",
                [spotify_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub fn set_deezer_artist(
        &mut self,
        spotify_id: &str,
        deezer_id: Option<i64>,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO deezer_artist (spotify_id, deezer_id, matched) VALUES (?1, ?2, ?3)
             ON CONFLICT(spotify_id) DO UPDATE
             SET deezer_id = excluded.deezer_id, matched = excluded.matched",
            params![spotify_id, deezer_id, now],
        )?;
        Ok(())
    }

    /// Barcodes and titles of an artist's albums that Spotify told about
    /// (saved albums, discographies), and titles of library albums by
    /// the same name.
    pub fn known_albums(&self, spotify_id: &str, name: &str) -> Result<KnownAlbums> {
        let mut known = KnownAlbums::default();
        let mut stmt = self.conn.prepare_cached(
            "SELECT name, upc FROM spotify_album
             WHERE ',' || artist_ids || ',' LIKE '%,' || ?1 || ',%'
             ORDER BY upc IS NULL, saved_position IS NULL",
        )?;
        for row in stmt.query_map([spotify_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })? {
            let (title, upc) = row?;
            known.titles.push(title);
            known.upcs.extend(upc);
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.title FROM album a JOIN album_artist aa ON aa.album_id = a.id
             JOIN artist ar ON ar.id = aa.artist_id
             WHERE aa.role = 'credit' AND ar.name = ?1 COLLATE NOCASE",
        )?;
        for title in stmt.query_map([name], |row| row.get::<_, String>(0))? {
            known.titles.push(title?);
        }
        Ok(known)
    }

    /// Replaces an artist's recent releases. Barcodes and Spotify IDs found
    /// earlier for releases that stay are kept, and so are releases another
    /// followed artist lists too.
    pub fn replace_new_releases(
        &mut self,
        artist_spotify_id: &str,
        releases: &[NewRelease],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let stored: Vec<i64> = tx
            .prepare("SELECT deezer_id FROM new_release_artist WHERE artist_spotify_id = ?1")?
            .query_map([artist_spotify_id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in stored {
            if !releases.iter().any(|r| r.deezer_id == id) {
                tx.execute(
                    "DELETE FROM new_release_artist
                     WHERE deezer_id = ?1 AND artist_spotify_id = ?2",
                    params![id, artist_spotify_id],
                )?;
            }
        }
        for r in releases {
            tx.execute(
                "INSERT INTO new_release (deezer_id, title, kind, release_date, image_url, upc,
                     spotify_album_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(deezer_id) DO UPDATE SET
                     title = excluded.title, kind = excluded.kind,
                     release_date = excluded.release_date, image_url = excluded.image_url,
                     upc = COALESCE(excluded.upc, new_release.upc),
                     spotify_album_id = COALESCE(excluded.spotify_album_id,
                                                 new_release.spotify_album_id)",
                params![
                    r.deezer_id,
                    r.title,
                    r.kind,
                    r.release_date,
                    r.image_url,
                    r.upc,
                    r.spotify_album_id
                ],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO new_release_artist (deezer_id, artist_spotify_id)
                 VALUES (?1, ?2)",
                params![r.deezer_id, artist_spotify_id],
            )?;
        }
        remove_unlisted(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Forgets the releases of artists no longer followed (a release
    /// another followed artist lists too stays).
    pub fn remove_unfollowed_releases(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM new_release_artist WHERE artist_spotify_id NOT IN
             (SELECT spotify_id FROM spotify_artist WHERE followed_position IS NOT NULL)",
            [],
        )?;
        remove_unlisted(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Releases of followed artists from `since` on (YYYY-MM-DD), newest
    /// first. A release by several followed artists comes once, with their
    /// names in the order they're followed in.
    pub fn new_releases(&self, since: &str) -> Result<Vec<NewRelease>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT r.deezer_id,
                    group_concat(f.spotify_id, ',' ORDER BY f.followed_position),
                    group_concat(f.name, ', ' ORDER BY f.followed_position) AS artists,
                    r.title, r.kind, r.release_date, r.image_url, r.upc, r.spotify_album_id,
                    r.seen
             FROM new_release r
             JOIN new_release_artist a ON a.deezer_id = r.deezer_id
             JOIN spotify_artist f ON f.spotify_id = a.artist_spotify_id
             WHERE f.followed_position IS NOT NULL AND r.release_date >= ?1
             GROUP BY r.deezer_id
             ORDER BY r.release_date DESC, artists COLLATE NOCASE, r.title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([since], |row| {
            let ids: String = row.get(1)?;
            Ok(NewRelease {
                deezer_id: row.get(0)?,
                artist_spotify_id: ids.split(',').next().unwrap_or_default().to_owned(),
                artist: row.get(2)?,
                title: row.get(3)?,
                kind: row.get(4)?,
                release_date: row.get(5)?,
                image_url: row.get(6)?,
                upc: row.get(7)?,
                spotify_album_id: row.get(8)?,
                seen: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The releases from `since` on have been shown.
    pub fn mark_new_releases_seen(&mut self, since: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE new_release SET seen = 1 WHERE release_date >= ?1",
            [since],
        )?;
        Ok(())
    }

    /// A release's barcode, once read from Deezer.
    pub fn set_new_release_upc(&mut self, deezer_id: i64, upc: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE new_release SET upc = ?2 WHERE deezer_id = ?1",
            params![deezer_id, upc],
        )?;
        Ok(())
    }

    /// The release on Spotify, once found.
    pub fn set_new_release_spotify(&mut self, deezer_id: i64, spotify_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE new_release SET spotify_album_id = ?2 WHERE deezer_id = ?1",
            params![deezer_id, spotify_id],
        )?;
        Ok(())
    }
}

/// Releases no followed artist lists any more.
fn remove_unlisted(tx: &rusqlite::Transaction) -> Result<()> {
    tx.execute(
        "DELETE FROM new_release WHERE deezer_id NOT IN
         (SELECT deezer_id FROM new_release_artist)",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{SpotifyAlbum, SpotifyArtist};

    fn release(id: i64, artist: &str, title: &str, date: &str) -> NewRelease {
        NewRelease {
            deezer_id: id,
            artist_spotify_id: artist.into(),
            artist: artist.to_uppercase(),
            title: title.into(),
            kind: "single".into(),
            release_date: date.into(),
            ..NewRelease::default()
        }
    }

    fn followed(db: &mut Database, ids: &[&str]) {
        let artists: Vec<SpotifyArtist> = ids
            .iter()
            .map(|id| SpotifyArtist {
                spotify_id: (*id).into(),
                name: id.to_uppercase(),
                ..SpotifyArtist::default()
            })
            .collect();
        db.replace_followed_artists(&artists).unwrap();
    }

    #[test]
    fn new_releases_are_stored_per_artist() {
        let mut db = Database::open_in_memory().unwrap();
        followed(&mut db, &["a", "b"]);
        db.replace_new_releases(
            "a",
            &[
                release(1, "a", "Old", "2026-09-01"),
                release(2, "a", "New", "2026-11-01"),
            ],
        )
        .unwrap();
        db.replace_new_releases("b", &[release(3, "b", "Other", "2026-10-20")])
            .unwrap();
        db.set_new_release_upc(2, "123").unwrap();
        db.set_new_release_spotify(2, "sp2").unwrap();
        let titles = |db: &Database, since| {
            db.new_releases(since)
                .unwrap()
                .into_iter()
                .map(|r| r.title)
                .collect::<Vec<_>>()
        };
        // Newest first, from the date on.
        assert_eq!(titles(&db, "2026-10-01"), ["New", "Other"]);
        assert_eq!(titles(&db, "2026-01-01"), ["New", "Other", "Old"]);

        assert!(
            db.new_releases("2026-01-01")
                .unwrap()
                .iter()
                .all(|r| !r.seen)
        );
        db.mark_new_releases_seen("2026-01-01").unwrap();

        // Read again: one gone, one new; what was found for "New" stays,
        // also that it was seen; the new one isn't.
        db.replace_new_releases(
            "a",
            &[
                release(2, "a", "New", "2026-11-01"),
                release(4, "a", "Newer", "2026-11-10"),
            ],
        )
        .unwrap();
        assert_eq!(titles(&db, "2026-01-01"), ["Newer", "New", "Other"]);
        let new = &db.new_releases("2026-11-01").unwrap()[1];
        assert_eq!(new.upc.as_deref(), Some("123"));
        assert_eq!(new.spotify_album_id.as_deref(), Some("sp2"));
        assert!(new.seen);
        assert!(!db.new_releases("2026-11-01").unwrap()[0].seen);

        // Unfollowed: not shown, and forgotten.
        followed(&mut db, &["a"]);
        assert_eq!(titles(&db, "2026-01-01"), ["Newer", "New"]);
        db.remove_unfollowed_releases().unwrap();
        followed(&mut db, &["a", "b"]);
        assert_eq!(titles(&db, "2026-01-01"), ["Newer", "New"]);
    }

    #[test]
    fn a_release_by_two_followed_artists_stays_while_one_is_followed() {
        let mut db = Database::open_in_memory().unwrap();
        followed(&mut db, &["a", "b"]);
        db.replace_new_releases("a", &[release(5, "a", "Duet", "2026-11-01")])
            .unwrap();
        db.replace_new_releases(
            "b",
            &[
                release(5, "b", "Duet", "2026-11-01"),
                release(6, "b", "Solo", "2026-10-20"),
            ],
        )
        .unwrap();
        db.mark_new_releases_seen("2026-01-01").unwrap();
        let shown = |db: &Database| {
            db.new_releases("2026-01-01")
                .unwrap()
                .into_iter()
                .map(|r| (r.title, r.artist, r.seen))
                .collect::<Vec<_>>()
        };
        // Once, with both names, in the order they're followed in.
        assert_eq!(
            shown(&db),
            [
                ("Duet".to_owned(), "A, B".to_owned(), true),
                ("Solo".to_owned(), "B".to_owned(), true),
            ]
        );

        // B unfollowed: the duet stays, A's, still seen.
        followed(&mut db, &["a"]);
        db.remove_unfollowed_releases().unwrap();
        followed(&mut db, &["a", "b"]);
        assert_eq!(shown(&db), [("Duet".to_owned(), "A".to_owned(), true)]);

        // A's list no longer has it: gone.
        db.replace_new_releases("a", &[]).unwrap();
        assert!(shown(&db).is_empty());
    }

    #[test]
    fn releases_stored_with_one_artist_keep_it() {
        use rusqlite_migration::Migrations;
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        // Up to 029_new_releases.sql.
        Migrations::from_slice(&crate::db::MIGRATIONS[..29])
            .to_latest(&mut conn)
            .unwrap();
        conn.execute_batch(
            "INSERT INTO new_release (deezer_id, artist_spotify_id, artist, title, kind,
                 release_date, upc, seen)
             VALUES (5, 'a', 'A', 'Duet', 'single', '2026-11-01', '0005', 1)",
        )
        .unwrap();
        Migrations::from_slice(crate::db::MIGRATIONS)
            .to_latest(&mut conn)
            .unwrap();
        let row: (i64, String, Option<String>, bool) = conn
            .query_row(
                "SELECT r.deezer_id, a.artist_spotify_id, r.upc, r.seen
                 FROM new_release r JOIN new_release_artist a USING (deezer_id)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (5, "a".to_owned(), Some("0005".to_owned()), true));
    }

    #[test]
    fn deezer_artists_and_known_albums() {
        let mut db = Database::open_in_memory().unwrap();
        assert_eq!(db.deezer_artist("a").unwrap(), None);
        db.set_deezer_artist("a", Some(42), 100).unwrap();
        db.set_deezer_artist("b", None, 100).unwrap();
        db.set_deezer_artist("a", Some(43), 200).unwrap();
        assert_eq!(db.deezer_artist("a").unwrap(), Some((Some(43), 200)));
        assert_eq!(db.deezer_artist("b").unwrap(), Some((None, 100)));

        let album = |id: &str, name: &str, artists: &[&str], upc: Option<&str>| SpotifyAlbum {
            spotify_id: id.into(),
            name: name.into(),
            artist_ids: artists.iter().map(|a| (*a).into()).collect(),
            album_type: "album".into(),
            upc: upc.map(Into::into),
            ..SpotifyAlbum::default()
        };
        db.replace_saved_albums(&[
            album("1", "With Barcode", &["a"], Some("0001")),
            album("2", "Duet", &["b", "a"], None),
            album("3", "Someone Else", &["ab"], Some("0003")),
        ])
        .unwrap();
        let known = db.known_albums("a", "Nobody").unwrap();
        assert_eq!(known.upcs, ["0001"]);
        assert_eq!(known.titles, ["With Barcode", "Duet"]);
    }
}
