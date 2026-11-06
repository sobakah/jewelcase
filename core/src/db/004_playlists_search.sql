-- Local playlists (CONCEPT.md §9.5, already shaped for mixed playlists)
-- and full-text search (§4.1).

CREATE TABLE playlist (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    -- 'local'; 'spotify' mirrors come in phase 7.
    source TEXT NOT NULL DEFAULT 'local',
    custom_cover_key TEXT,
    created INTEGER NOT NULL,
    modified INTEGER NOT NULL
);

-- An entry keeps a snapshot of what it points to, so it can be re-linked
-- when files move (and matched against Spotify later).
CREATE TABLE playlist_item (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    playlist_id INTEGER NOT NULL REFERENCES playlist(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    local_track_id INTEGER REFERENCES track(id) ON DELETE SET NULL,
    spotify_uri TEXT,
    isrc TEXT,
    title TEXT,
    artist TEXT,
    album TEXT,
    duration_ms INTEGER,
    path TEXT,
    mb_track_id TEXT,
    -- 'local' | 'unavailable'; Spotify states come in phase 7.
    match_state TEXT NOT NULL
);

CREATE INDEX playlist_item_playlist ON playlist_item(playlist_id, position);
CREATE INDEX playlist_item_track ON playlist_item(local_track_id);

-- Trigram: substring matches, also in Hangul, Kana and Han text without
-- spaces; diacritics are ignored ("beyonce" finds Beyoncé).
CREATE VIRTUAL TABLE search USING fts5(
    kind UNINDEXED,
    ref_id UNINDEXED,
    text,
    tokenize = 'trigram remove_diacritics 1'
);
