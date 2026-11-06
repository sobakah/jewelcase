-- Recently played songs, shown above "Now Playing" in the queue (§6.2).
CREATE TABLE play_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    track_id INTEGER NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    played INTEGER NOT NULL
);
CREATE INDEX play_history_track ON play_history(track_id);
