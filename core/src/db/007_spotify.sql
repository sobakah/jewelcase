-- Spotify playlists (CONCEPT.md §9.5, §9.6): mirrors of the user's Spotify
-- playlists, and local playlists published there. Both are "linked": the
-- entries with a Spotify URI are kept in sync, the others stay local.

ALTER TABLE playlist ADD COLUMN spotify_id TEXT;
ALTER TABLE playlist ADD COLUMN snapshot_id TEXT;
ALTER TABLE playlist ADD COLUMN spotify_owner TEXT;
-- Owned or collaborative: may be changed here (§9.6).
ALTER TABLE playlist ADD COLUMN editable INTEGER NOT NULL DEFAULT 1;
-- 0: Spotify doesn't show the entries (others' playlists, §9.5).
ALTER TABLE playlist ADD COLUMN readable INTEGER NOT NULL DEFAULT 1;
ALTER TABLE playlist ADD COLUMN spotify_image_url TEXT;
ALTER TABLE playlist ADD COLUMN spotify_cover_key TEXT;
CREATE UNIQUE INDEX playlist_spotify ON playlist(spotify_id);

-- Unconfirmed matches: the library song that is probably this Spotify
-- entry, or (JSON) the Spotify track that is probably this library song.
ALTER TABLE playlist_item ADD COLUMN suggested_track_id INTEGER
    REFERENCES track(id) ON DELETE SET NULL;
ALTER TABLE playlist_item ADD COLUMN suggested_spotify TEXT;
CREATE INDEX playlist_item_uri ON playlist_item(spotify_uri);

-- Playlist changes waiting to be sent, in order (§9.6).
CREATE TABLE spotify_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    playlist_id INTEGER NOT NULL REFERENCES playlist(id) ON DELETE CASCADE,
    op TEXT NOT NULL,
    created INTEGER NOT NULL
);

-- The Spotify version of a library song, or that none was found; so each
-- song is searched for once.
CREATE TABLE spotify_track (
    track_id INTEGER PRIMARY KEY REFERENCES track(id) ON DELETE CASCADE,
    uri TEXT,
    -- 'sure' | 'likely'; NULL when nothing was found.
    confidence TEXT,
    title TEXT,
    artist TEXT,
    checked INTEGER NOT NULL
);
