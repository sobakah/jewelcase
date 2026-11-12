-- New releases of followed Spotify artists, read from Deezer (CONCEPT.md
-- §9.8): Spotify's own artist requests are too limited to ask for every
-- followed artist.

-- Each followed artist's Deezer artist.
CREATE TABLE deezer_artist (
    spotify_id TEXT PRIMARY KEY,
    -- NULL: not found on Deezer.
    deezer_id INTEGER,
    -- When it was looked for (Unix seconds).
    matched INTEGER NOT NULL
);

-- Their recent releases on Deezer.
CREATE TABLE new_release (
    deezer_id INTEGER PRIMARY KEY,
    artist_spotify_id TEXT NOT NULL,
    artist TEXT NOT NULL,
    title TEXT NOT NULL,
    -- 'album', 'ep' or 'single'.
    kind TEXT NOT NULL,
    -- YYYY-MM-DD.
    release_date TEXT NOT NULL,
    image_url TEXT,
    upc TEXT,
    -- The same release on Spotify, once found there.
    spotify_album_id TEXT
);
CREATE INDEX new_release_artist ON new_release(artist_spotify_id);
