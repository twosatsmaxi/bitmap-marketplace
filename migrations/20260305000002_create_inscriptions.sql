CREATE TABLE inscriptions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inscription_id TEXT NOT NULL UNIQUE,
    inscription_number BIGINT NOT NULL,
    content_type TEXT,
    content_length BIGINT,
    owner_address TEXT NOT NULL,
    sat_ordinal BIGINT,
    genesis_block_height BIGINT,
    genesis_timestamp TIMESTAMPTZ,
    collection_id UUID REFERENCES collections(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_inscriptions_owner ON inscriptions(owner_address);
CREATE INDEX idx_inscriptions_collection ON inscriptions(collection_id);
CREATE INDEX idx_inscriptions_number ON inscriptions(inscription_number);
