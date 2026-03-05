CREATE TYPE listing_status AS ENUM ('active', 'sold', 'cancelled', 'expired');

CREATE TABLE listings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inscription_id TEXT NOT NULL,
    seller_address TEXT NOT NULL,
    price_sats BIGINT NOT NULL CHECK (price_sats > 0),
    status listing_status NOT NULL DEFAULT 'active',
    psbt TEXT,
    royalty_address TEXT,
    royalty_bps INTEGER CHECK (royalty_bps BETWEEN 0 AND 10000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_listings_inscription ON listings(inscription_id);
CREATE INDEX idx_listings_seller ON listings(seller_address);
CREATE INDEX idx_listings_status ON listings(status);
CREATE INDEX idx_listings_price ON listings(price_sats) WHERE status = 'active';
