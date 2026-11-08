-- Listening statistics (CONCEPT.md §6.3): every play, kept until the
-- user deletes it (the history keeps 200 and can be cleared). What
-- played is copied in, so the statistics survive rescans and removed
-- songs.
CREATE TABLE play_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Unix seconds, when the song was left.
    played INTEGER NOT NULL,
    ms_played INTEGER NOT NULL,
    -- A library song, or a streamed one as -stream_track.id.
    track_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    artist TEXT,
    album TEXT,
    album_artist TEXT,
    genre TEXT
);
CREATE INDEX play_log_played ON play_log(played);

-- The history so far starts it (how long each played isn't known: the
-- whole song).
INSERT INTO play_log (played, ms_played, track_id, title, artist, album, album_artist, genre)
    SELECT h.played, t.duration_ms, h.track_id, coalesce(t.title, ''), t.artist, t.album,
           coalesce(t.album_artist, t.artist),
           nullif(substr(t.genres, 1, instr(t.genres || char(31), char(31)) - 1), '')
    FROM play_history h JOIN track t ON t.id = h.track_id
    UNION ALL
    SELECT h.played, s.duration_ms, h.track_id, s.title, s.artist, s.album, s.artist, NULL
    FROM play_history h JOIN stream_track s ON s.id = -h.track_id
    ORDER BY 1;
