-- Streamed Spotify songs go into "Recently Played" too (negative IDs:
-- -stream_track.id), so the history can't reference track(id) any more.
-- Removed library songs still leave it, through a trigger.
CREATE TABLE play_history_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    track_id INTEGER NOT NULL,
    played INTEGER NOT NULL
);
INSERT INTO play_history_new (id, track_id, played)
    SELECT id, track_id, played FROM play_history;
DROP TABLE play_history;
ALTER TABLE play_history_new RENAME TO play_history;
CREATE INDEX play_history_track ON play_history(track_id);
CREATE TRIGGER play_history_track_removed AFTER DELETE ON track
BEGIN
    DELETE FROM play_history WHERE track_id = OLD.id;
END;
