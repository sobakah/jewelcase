-- Each song's measured loudness (LUFS of the mono analysis signal), so
-- songs without ReplayGain tags play as loud as tagged ones and Spotify.
-- Filled by the analysis (features version 4; older results are analyzed
-- again).
ALTER TABLE analysis ADD COLUMN loudness REAL;
