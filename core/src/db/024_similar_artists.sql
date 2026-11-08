-- Related artists from ListenBrainz (its similar-artists dataset, by
-- MusicBrainz ID), for smart shuffle's "same artist" criterion (§7).
CREATE TABLE similar_artist (
    artist_mbid TEXT NOT NULL,
    similar_mbid TEXT NOT NULL,
    score INTEGER NOT NULL,
    PRIMARY KEY (artist_mbid, similar_mbid)
);

-- Artists asked for, and when (also when ListenBrainz had nothing).
CREATE TABLE similar_artist_checked (
    artist_mbid TEXT PRIMARY KEY,
    checked INTEGER NOT NULL
);
