-- A playlist shows only the songs that play from the library ("Only
-- Songs in My Library" in its sort menu, CONCEPT.md §4.1). Only the
-- view, remembered per playlist like its sort.
ALTER TABLE playlist ADD COLUMN library_only INTEGER NOT NULL DEFAULT 0;
