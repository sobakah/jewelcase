-- Spotify and Deezer became portrait sources (fanart.tv was dropped):
-- finished lookups are due once more, for their better portraits. Only
-- the automatic portrait changes; one the user chose stays.
UPDATE artist SET looked_up = 0 WHERE lookup_state = 'done';
