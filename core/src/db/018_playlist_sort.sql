-- How a playlist is listed and played (CONCEPT.md §4.1): 'custom' is its
-- own order; or by title, artist, album, date added or length; any of
-- them reversed. Only the view: the stored order stays.
ALTER TABLE playlist ADD COLUMN sort_by TEXT NOT NULL DEFAULT 'custom';
ALTER TABLE playlist ADD COLUMN sort_reversed INTEGER NOT NULL DEFAULT 0;

-- When an entry was added (Unix seconds): Spotify's date for Spotify
-- entries, else the moment it was inserted. Unknown for older entries.
ALTER TABLE playlist_item ADD COLUMN added INTEGER;

CREATE TRIGGER playlist_item_added AFTER INSERT ON playlist_item
WHEN NEW.added IS NULL
BEGIN
    UPDATE playlist_item SET added = CAST(strftime('%s', 'now') AS INTEGER)
    WHERE id = NEW.id;
END;
