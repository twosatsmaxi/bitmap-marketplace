-- Add mempool protection columns to listings table.
-- seller_pubkey: compressed secp256k1 public key (hex) provided at listing time.
-- multisig_address: P2WSH 2-of-2 address where inscription is locked.
-- multisig_script: hex-encoded P2WSH redeem script (witness script).
-- locking_raw_tx: signed raw transaction hex that moves inscription into multisig.
--                 Stored but NOT broadcast until purchase confirmation.
-- protection_status: tracks locking lifecycle.

ALTER TABLE listings
    ADD COLUMN seller_pubkey TEXT,
    ADD COLUMN multisig_address TEXT,
    ADD COLUMN multisig_script TEXT,
    ADD COLUMN locking_raw_tx TEXT,
    ADD COLUMN protection_status TEXT NOT NULL DEFAULT 'none'
        CHECK (protection_status IN ('none', 'locking_pending', 'active'));

-- Add locking_tx_id to sales to track the package broadcast.
ALTER TABLE sales
    ADD COLUMN locking_tx_id TEXT;
