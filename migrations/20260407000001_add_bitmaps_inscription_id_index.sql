-- Add index on inscription_id to speed up portfolio queries.
-- Portfolio endpoints do WHERE inscription_id = ANY($1) which causes full
-- table scans (~2s) without this index.
--
-- The bitmaps table is created and populated externally (data import pipeline),
-- so guard to avoid failures in fresh/test environments.
DO $$ BEGIN
IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'bitmaps') THEN
    CREATE INDEX IF NOT EXISTS idx_bitmaps_inscription_id ON bitmaps (inscription_id);
    ANALYZE bitmaps;
END IF;
END $$;
