-- An artist's most popular songs on Deezer, for their page's Popular
-- section (CONCEPT.md §4.2), kept a day: by their Deezer artist.
CREATE TABLE deezer_popular (
    artist_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    song_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT,
    duration_ms INTEGER NOT NULL,
    cover_url TEXT,
    PRIMARY KEY (artist_id, position)
);

-- When each artist's list was read (Unix seconds).
CREATE TABLE deezer_popular_read (
    artist_id INTEGER PRIMARY KEY,
    read INTEGER NOT NULL
);
