-- Spotify playlists read before migration 008 have no album images for
-- their entries (covers of streamed songs): read them all once more.
UPDATE playlist SET snapshot_id = NULL WHERE spotify_id IS NOT NULL;
