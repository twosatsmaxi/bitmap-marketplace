CREATE TABLE sales (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    listing_id UUID REFERENCES listings(id),
    inscription_id TEXT NOT NULL,
    seller_address TEXT NOT NULL,
    buyer_address TEXT NOT NULL,
    price_sats BIGINT NOT NULL,
    royalty_sats BIGINT NOT NULL DEFAULT 0,
    tx_id TEXT,
    block_height BIGINT,
    confirmed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_sales_inscription ON sales(inscription_id);
CREATE INDEX idx_sales_seller ON sales(seller_address);
CREATE INDEX idx_sales_buyer ON sales(buyer_address);
CREATE INDEX idx_sales_tx ON sales(tx_id);
