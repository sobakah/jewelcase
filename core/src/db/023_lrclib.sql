-- Lyrics from LRCLIB for Spotify songs that aren't in the library, by
-- Spotify URI; also what LRCLIB didn't have (found = 0), so it isn't
-- asked again soon. Shown only, never written to files.
CREATE TABLE online_lyrics (
    uri TEXT PRIMARY KEY,
    checked INTEGER NOT NULL,
    found INTEGER NOT NULL,
    synced TEXT,
    plain TEXT,
    instrumental INTEGER NOT NULL DEFAULT 0
);
