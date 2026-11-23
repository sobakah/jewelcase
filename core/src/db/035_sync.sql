-- The sync with Jewelcase Mobile through sync files (CONCEPT.md §6.4; Jewelcase Mobile's
-- docs/SYNC.md): when a choice for a Spotify song was made, choices forgotten, plays read from
-- the phone (and those deleted), and local playlists' UUIDs and deletions.
ALTER TABLE playlist_item ADD COLUMN choice_at INTEGER;
CREATE TABLE forgotten_choice (
    uri TEXT PRIMARY KEY,
    -- Unix ms.
    at INTEGER NOT NULL
);
ALTER TABLE play_log ADD COLUMN remote_id TEXT;
CREATE UNIQUE INDEX play_log_remote ON play_log(remote_id);
CREATE TABLE skipped_play (remote_id TEXT PRIMARY KEY);
ALTER TABLE playlist ADD COLUMN sync_id TEXT;
CREATE UNIQUE INDEX playlist_sync ON playlist(sync_id);
CREATE TABLE playlist_deleted (
    sync_id TEXT PRIMARY KEY,
    -- Unix ms.
    at INTEGER NOT NULL
);
