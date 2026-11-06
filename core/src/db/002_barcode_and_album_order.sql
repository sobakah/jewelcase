-- Barcodes for the split-release rule (grouping.rs), and each track's place
-- and disc in its album as computed by the grouping.

ALTER TABLE track ADD COLUMN barcode TEXT;
ALTER TABLE track ADD COLUMN album_position INTEGER;
ALTER TABLE track ADD COLUMN album_disc INTEGER;

-- Reread every file once, so barcodes get filled in.
UPDATE track SET mtime = -1;
