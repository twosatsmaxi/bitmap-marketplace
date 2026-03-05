use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Collection {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub royalty_address: Option<String>,
    pub royalty_bps: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionStats {
    pub collection_id: Uuid,
    pub floor_price_sats: Option<i64>,
    pub total_volume_sats: i64,
    pub listed_count: i64,
    pub total_supply: i64,
    pub owners_count: i64,
}
