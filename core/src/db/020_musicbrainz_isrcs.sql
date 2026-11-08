-- ISRCs from MusicBrainz for songs whose files have none (CONCEPT.md
-- §9.5, step 2), by release (the album's MusicBrainz ID), disc and track
-- number: one request per album. Only used to match Spotify entries;
-- the files and their tags stay as they are.
CREATE TABLE mb_isrc (
    release TEXT NOT NULL,
    disc INTEGER NOT NULL,
    number INTEGER NOT NULL,
    isrc TEXT NOT NULL,
    PRIMARY KEY (release, disc, number)
);

-- Releases asked for, and when (also when MusicBrainz had nothing).
CREATE TABLE mb_isrc_release (
    release TEXT PRIMARY KEY,
    checked INTEGER NOT NULL
);
