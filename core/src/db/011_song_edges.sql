-- Where each song's music starts and ends, for smart crossfades. Filled by
-- the analysis (features version 3; older results are analyzed again).
ALTER TABLE analysis ADD COLUMN lead_in_ms INTEGER;
ALTER TABLE analysis ADD COLUMN outro_ms INTEGER;
ALTER TABLE analysis ADD COLUMN end_ms INTEGER;
