-- "Play … from your library?" answered with No: that library song isn't
-- suggested for the entry again (rematches and re-reads kept asking).
ALTER TABLE playlist_item ADD COLUMN declined_track_id INTEGER;
