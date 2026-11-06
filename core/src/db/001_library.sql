-- Library schema, roadmap phase 1 (CONCEPT.md §3.2, §6.1).

CREATE TABLE album (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    group_key TEXT NOT NULL,
    title TEXT NOT NULL,
    sort_title TEXT NOT NULL,
    edition TEXT,
    artist_credit TEXT NOT NULL,
    artist_sort TEXT NOT NULL,
    year INTEGER,
    release_type TEXT NOT NULL,
    soundtrack INTEGER NOT NULL,
    compilation INTEGER NOT NULL,
    mb_album_id TEXT,
    mb_release_group_id TEXT,
    track_count INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    cover_key TEXT,
    added INTEGER NOT NULL
);

-- AUTOINCREMENT: the queue refers to tracks by ID, so IDs are never reused.
CREATE TABLE track (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    mtime INTEGER NOT NULL,
    size INTEGER NOT NULL,
    added INTEGER NOT NULL,
    album_id INTEGER REFERENCES album(id) ON DELETE SET NULL,
    title TEXT,
    artist TEXT,
    album TEXT,
    album_artist TEXT,
    album_artists TEXT NOT NULL,
    album_artist_sort TEXT,
    disc INTEGER,
    number INTEGER,
    year INTEGER,
    genres TEXT NOT NULL,
    composer TEXT,
    mb_album_id TEXT,
    mb_release_group_id TEXT,
    mb_track_id TEXT,
    mb_album_artist_ids TEXT NOT NULL,
    release_comment TEXT,
    release_type TEXT,
    compilation INTEGER NOT NULL,
    isrc TEXT,
    bpm REAL,
    lyrics_synced INTEGER,
    lyrics_excerpt TEXT,
    replaygain_track REAL,
    replaygain_album REAL,
    duration_ms INTEGER NOT NULL,
    format TEXT NOT NULL,
    bitrate INTEGER,
    sample_rate INTEGER,
    bit_depth INTEGER,
    channels INTEGER,
    cover_key TEXT
);

CREATE INDEX track_album ON track(album_id);

-- Only album artists get entries (§3.3). The full artist model is phase 2.
CREATE TABLE artist (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    name_key TEXT NOT NULL UNIQUE,
    sort_name TEXT NOT NULL,
    mbid TEXT
);

CREATE TABLE album_artist (
    album_id INTEGER NOT NULL REFERENCES album(id) ON DELETE CASCADE,
    artist_id INTEGER NOT NULL REFERENCES artist(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (album_id, position)
);

CREATE TABLE queue_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    current INTEGER,
    position_ms INTEGER NOT NULL,
    shuffle INTEGER NOT NULL,
    repeat TEXT NOT NULL,
    context_album INTEGER
);

-- No foreign key: tracks can disappear while queued; restoring skips them.
CREATE TABLE queue_item (
    position INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL,
    origin TEXT NOT NULL,
    unshuffled_position INTEGER
);
