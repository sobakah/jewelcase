-- The user's saved albums and followed artists on Spotify, and artists'
-- discographies there (CONCEPT.md §9.8). Kept apart from the library.
CREATE TABLE spotify_album (
    spotify_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    -- "Artist, Artist", and their Spotify IDs separated by commas.
    artist TEXT,
    artist_ids TEXT,
    -- 'album', 'single' (singles and EPs) or 'compilation'.
    album_type TEXT NOT NULL,
    year INTEGER,
    total_tracks INTEGER NOT NULL DEFAULT 0,
    image_url TEXT,
    upc TEXT,
    -- Order among the saved albums (0: saved last); NULL: not saved.
    saved_position INTEGER
);
CREATE INDEX spotify_album_saved ON spotify_album(saved_position);

CREATE TABLE spotify_artist (
    spotify_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    image_url TEXT,
    -- Separated by commas.
    genres TEXT,
    -- Order among the followed artists; NULL: not followed.
    followed_position INTEGER
);

-- An artist's releases on Spotify, in Spotify's order.
CREATE TABLE spotify_discography (
    artist_id TEXT NOT NULL,
    album_id TEXT NOT NULL REFERENCES spotify_album(spotify_id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (artist_id, album_id)
);

-- When each discography was read (they're read again after a day).
CREATE TABLE spotify_discography_read (
    artist_id TEXT PRIMARY KEY,
    read INTEGER NOT NULL
);
