CREATE TYPE activity_type AS ENUM ('list', 'delist', 'sale', 'transfer', 'mint');

CREATE TABLE activity_feed (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inscription_id TEXT NOT NULL,
    collection_id UUID REFERENCES collections(id),
    activity_type activity_type NOT NULL,
    from_address TEXT,
    to_address TEXT,
    price_sats BIGINT,
    tx_id TEXT,
    block_height BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_activity_inscription ON activity_feed(inscription_id);
CREATE INDEX idx_activity_collection ON activity_feed(collection_id);
CREATE INDEX idx_activity_type ON activity_feed(activity_type);
CREATE INDEX idx_activity_created ON activity_feed(created_at DESC);
