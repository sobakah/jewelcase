-- Spotify album images are now taken at their largest (640 px instead of
-- 300 px): read the Spotify playlists once more for the new links.
UPDATE playlist SET snapshot_id = NULL WHERE spotify_id IS NOT NULL AND readable = 1;
