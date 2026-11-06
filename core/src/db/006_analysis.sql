-- Song analysis for smart shuffle (§7), by file content (analysis.rs), so
-- moved or renamed files and identical files in editions share it.
CREATE TABLE analysis (
    key TEXT PRIMARY KEY,
    features BLOB,
    features_version INTEGER,
    bpm REAL,
    -- The file couldn't be analyzed; not tried again until it changes.
    failed INTEGER NOT NULL DEFAULT 0,
    analyzed INTEGER NOT NULL
);

-- The file's content key, once computed (cleared when the file changes).
ALTER TABLE track ADD COLUMN content_key TEXT;
CREATE INDEX track_content_key ON track(content_key);
