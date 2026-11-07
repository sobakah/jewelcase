-- Lookups that ran over the whole table: matching playlist entries to
-- songs (every scan, for each entry not in the library) and the foreign
-- keys set to NULL when a song is deleted.
CREATE INDEX track_mb_track_id ON track(mb_track_id);
CREATE INDEX track_isrc ON track(isrc);
CREATE INDEX track_title_lower ON track(lower(title));
CREATE INDEX playlist_item_suggested ON playlist_item(suggested_track_id);
CREATE INDEX playlist_item_declined ON playlist_item(declined_track_id);
