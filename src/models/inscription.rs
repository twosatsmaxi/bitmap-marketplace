use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Inscription {
    pub id: Uuid,
    pub inscription_id: String, // <txid>i<index>
    pub inscription_number: i64,
    pub content_type: Option<String>,
    pub content_length: Option<i64>,
    pub owner_address: String,
    pub sat_ordinal: Option<i64>,
    pub genesis_block_height: Option<i64>,
    pub genesis_timestamp: Option<DateTime<Utc>>,
    pub collection_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
