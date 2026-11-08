-- Romanization lyrembed made for display (§5.1), by a hash of the lyrics
-- text and the Han hint; lyrics = NULL when there was nothing to do.
-- Never written to files.
CREATE TABLE romanized (
    key TEXT PRIMARY KEY,
    lyrics TEXT,
    checked INTEGER NOT NULL
);
