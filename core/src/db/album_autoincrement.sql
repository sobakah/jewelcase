-- AUTOINCREMENT, as for tracks: the saved queue refers to its album by ID,
-- so a removed album's ID must not go to a new one. Rebuilt the way SQLite
-- describes for schema changes, with foreign keys off, so dropping the old
-- table leaves album_artist and track.album_id alone (Database::setup checks
-- the keys afterwards). Not a numbered migration: older versions open the
-- rebuilt table just the same, and an unchanged schema version lets them.
CREATE TABLE album_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
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
INSERT INTO album_new SELECT * FROM album;
DROP TABLE album;
ALTER TABLE album_new RENAME TO album;
