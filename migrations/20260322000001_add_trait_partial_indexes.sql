-- Partial B-tree indexes for trait-filtered explore queries.
-- The existing GIN index on traits can find matching rows but cannot provide
-- ORDER BY block_height for free, causing full table scans + sorts (~18s).
-- These partial indexes pre-filter by trait condition and are already sorted
-- by block_height, enabling index-only scans (< 10ms).

-- Grouped: all punk variants (11 traits) — used by filter=punks
CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_punk ON bitmaps (block_height)
WHERE traits && ARRAY['pristine_punk', 'perfect_punk', 'perfect_punk_7tx', 'perfect_punk_10tx',
                       'perfect_punk_13tx', 'perfect_punk_17tx', 'perfect_punk_21tx', 'perfect_punk_26tx',
                       'perfect_punk_43tx', 'standard_punk', 'wide_neck_punk']::text[];

-- Grouped: perfect punk variants (8 traits) — used by filter=perfect-punk
CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_perfect_punk ON bitmaps (block_height)
WHERE traits && ARRAY['perfect_punk', 'perfect_punk_7tx', 'perfect_punk_10tx', 'perfect_punk_13tx',
                       'perfect_punk_17tx', 'perfect_punk_21tx', 'perfect_punk_26tx', 'perfect_punk_43tx']::text[];

-- Individual trait indexes — used by their respective filters
CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_pristine_punk ON bitmaps (block_height)
WHERE traits @> ARRAY['pristine_punk']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_palindrome ON bitmaps (block_height)
WHERE traits @> ARRAY['palindrome']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_nakamoto ON bitmaps (block_height)
WHERE traits @> ARRAY['nakamoto']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_patoshi ON bitmaps (block_height)
WHERE traits @> ARRAY['patoshi']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_sub_100k ON bitmaps (block_height)
WHERE traits @> ARRAY['sub_100k']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_billionaire ON bitmaps (block_height)
WHERE traits @> ARRAY['billionaire']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_pizza ON bitmaps (block_height)
WHERE traits @> ARRAY['pizza']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_epic_sat ON bitmaps (block_height)
WHERE traits @> ARRAY['epic_sat']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_standard_punk ON bitmaps (block_height)
WHERE traits @> ARRAY['standard_punk']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_wide_neck_punk ON bitmaps (block_height)
WHERE traits @> ARRAY['wide_neck_punk']::text[];

CREATE INDEX IF NOT EXISTS idx_bitmaps_trait_repdigit ON bitmaps (block_height)
WHERE traits @> ARRAY['repdigit']::text[];

ANALYZE bitmaps;
