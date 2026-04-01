-- Store the seller's pre-signed partial signature for the sale template.
-- Produced at listing time when seller signs the sale template PSBT with
-- SIGHASH_SINGLE|ANYONECANPAY, embedded into the sale PSBT at buy time.
ALTER TABLE listings ADD COLUMN seller_sale_sig TEXT;
