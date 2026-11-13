-- A release can be by several followed artists (a duet, a sub-unit's
-- single): each of them lists it, and it stays while one of them is
-- followed. Before, the release had one artist, the last one whose list
-- was read.

CREATE TABLE new_release_moved (
    deezer_id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    -- 'album', 'ep' or 'single'.
    kind TEXT NOT NULL,
    -- YYYY-MM-DD.
    release_date TEXT NOT NULL,
    image_url TEXT,
    upc TEXT,
    -- The same release on Spotify, once found there.
    spotify_album_id TEXT,
    -- Shown in the feed (the bell loses its dot).
    seen INTEGER NOT NULL DEFAULT 0
);
INSERT INTO new_release_moved
    SELECT deezer_id, title, kind, release_date, image_url, upc, spotify_album_id, seen
    FROM new_release;
CREATE TABLE release_artist_moved AS
    SELECT deezer_id, artist_spotify_id FROM new_release;
DROP TABLE new_release;
ALTER TABLE new_release_moved RENAME TO new_release;

-- The followed artists whose releases list it.
CREATE TABLE new_release_artist (
    deezer_id INTEGER NOT NULL REFERENCES new_release(deezer_id) ON DELETE CASCADE,
    artist_spotify_id TEXT NOT NULL,
    PRIMARY KEY (deezer_id, artist_spotify_id)
) WITHOUT ROWID;
CREATE INDEX new_release_by_artist ON new_release_artist(artist_spotify_id);
INSERT INTO new_release_artist SELECT deezer_id, artist_spotify_id FROM release_artist_moved;
DROP TABLE release_artist_moved;
