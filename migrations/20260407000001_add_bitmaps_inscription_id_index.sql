-- Add index on inscription_id to speed up portfolio queries.
-- Portfolio endpoints do WHERE inscription_id = ANY($1) which causes full
-- table scans (~2s) without this index.
CREATE INDEX IF NOT EXISTS idx_bitmaps_inscription_id ON bitmaps (inscription_id);

ANALYZE bitmaps;
