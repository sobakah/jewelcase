-- An artist page reads only the first page of an artist's releases on
-- Spotify (Spotify allows few requests about artists a day); `total` is
-- Spotify's count of them all while only that page is stored, NULL when
-- all of them are (as before).
ALTER TABLE spotify_discography_read ADD COLUMN total INTEGER;
