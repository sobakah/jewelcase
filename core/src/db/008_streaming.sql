-- Spotify songs that aren't in the library, so they can be queued and
-- streamed (CONCEPT.md §9.4). Their track IDs are negative: -id here.
CREATE TABLE stream_track (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uri TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    artist TEXT,
    album TEXT,
    duration_ms INTEGER NOT NULL,
    isrc TEXT,
    image_url TEXT,
    cover_key TEXT
);

-- The album image of a Spotify entry, for its cover.
ALTER TABLE playlist_item ADD COLUMN image_url TEXT;
