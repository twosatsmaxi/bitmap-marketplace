use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Sale {
    pub id: Uuid,
    pub listing_id: Option<Uuid>,
    pub inscription_id: String,
    pub seller_address: String,
    pub buyer_address: String,
    pub price_sats: i64,
    pub marketplace_fee_sats: i64,
    pub tx_id: Option<String>,
    pub locking_tx_id: Option<String>,
    pub block_height: Option<i64>,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
