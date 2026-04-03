-- Replace royalty_sats with marketplace_fee_sats in sales table.
ALTER TABLE sales RENAME COLUMN royalty_sats TO marketplace_fee_sats;

-- Drop dead royalty columns from listings table.
ALTER TABLE listings DROP COLUMN IF EXISTS royalty_address;
ALTER TABLE listings DROP COLUMN IF EXISTS royalty_bps;
