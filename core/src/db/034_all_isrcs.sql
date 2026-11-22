-- Every ISRC of a song, not only the first (CONCEPT.md §9.5): a recording
-- often has several (a reissue, another label), and a Spotify copy may
-- carry any of them. track.isrc stays the first: shown, and what Spotify is
-- asked for.

ALTER TABLE track ADD COLUMN isrcs TEXT;

-- Reread the files that have an ISRC, so their others get filled in.
UPDATE track SET mtime = -1 WHERE isrc IS NOT NULL;

-- MusicBrainz' ISRCs: all of a recording's, so one row per ISRC. Asked for
-- again (one request per album), since only the first was kept.
DROP TABLE mb_isrc;
CREATE TABLE mb_isrc (
    release TEXT NOT NULL,
    disc INTEGER NOT NULL,
    number INTEGER NOT NULL,
    isrc TEXT NOT NULL,
    PRIMARY KEY (release, disc, number, isrc)
);
DELETE FROM mb_isrc_release;
