-- Songs stored without a length (FLAC files whose header doesn't say)
-- are read again on the next scan, which now finds it.
UPDATE track SET mtime = 0 WHERE duration_ms = 0;
