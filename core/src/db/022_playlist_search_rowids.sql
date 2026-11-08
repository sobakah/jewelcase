-- Playlists' search rows get a row ID derived from the playlist's ID
-- (2^40 + id, after every other row), so changing one doesn't scan the
-- whole index (kind and ref_id aren't indexed).
DELETE FROM search WHERE kind = 'playlist';
INSERT INTO search (rowid, kind, ref_id, text)
  SELECT 1099511627776 + id, 'playlist', id, name FROM playlist;
