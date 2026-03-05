CREATE TYPE offer_status AS ENUM ('pending', 'accepted', 'rejected', 'expired', 'cancelled');

CREATE TABLE offers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inscription_id TEXT NOT NULL,
    buyer_address TEXT NOT NULL,
    price_sats BIGINT NOT NULL CHECK (price_sats > 0),
    status offer_status NOT NULL DEFAULT 'pending',
    psbt TEXT,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_offers_inscription ON offers(inscription_id);
CREATE INDEX idx_offers_buyer ON offers(buyer_address);
CREATE INDEX idx_offers_status ON offers(status);
