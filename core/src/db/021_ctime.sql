-- The files' change time, so tag edits that keep the modification time
-- and size (Picard's "preserve timestamps") are noticed. Filled in by the
-- next scan without reading the files again.
ALTER TABLE track ADD COLUMN ctime INTEGER;
