-- Folder cover images (cover.jpg, folder.jpg …) as last seen, so a new,
-- replaced or removed one updates the songs' covers at the next scan.
CREATE TABLE folder_cover (
    folder TEXT PRIMARY KEY,
    image TEXT NOT NULL,
    mtime INTEGER NOT NULL,
    size INTEGER NOT NULL
);
