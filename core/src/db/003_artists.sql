-- The artist model (CONCEPT.md §3.3) and online artist metadata (§8).
-- The phase 1 artist tables held only names and are rebuilt on every scan.

DROP TABLE album_artist;
DROP TABLE artist;

-- AUTOINCREMENT: the UI and custom portraits refer to artists by ID.
CREATE TABLE artist (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    sort_name TEXT NOT NULL,
    -- From the tags.
    mbid TEXT,
    -- Online metadata, kept across rescans while the artist exists.
    lookup_state TEXT,
    looked_up INTEGER,
    lookup_mbid TEXT,
    wikidata_id TEXT,
    spotify_id TEXT,
    wikipedia_url TEXT,
    website TEXT,
    bio TEXT,
    portrait_key TEXT,
    portrait_source TEXT,
    portrait_attribution TEXT,
    custom_portrait_key TEXT
);

CREATE TABLE album_artist (
    album_id INTEGER NOT NULL REFERENCES album(id) ON DELETE CASCADE,
    artist_id INTEGER NOT NULL REFERENCES artist(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (album_id, artist_id)
);

CREATE INDEX album_artist_artist ON album_artist(artist_id);

-- "Appears on": tracks on other artists' albums.
CREATE TABLE track_artist (
    track_id INTEGER NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    artist_id INTEGER NOT NULL REFERENCES artist(id) ON DELETE CASCADE,
    PRIMARY KEY (track_id, artist_id)
);

CREATE INDEX track_artist_artist ON track_artist(artist_id);
