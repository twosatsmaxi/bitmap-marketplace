use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "activity_type", rename_all = "lowercase")]
pub enum ActivityType {
    List,
    Delist,
    Sale,
    Transfer,
    Mint,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Activity {
    pub id: Uuid,
    pub inscription_id: String,
    pub collection_id: Option<Uuid>,
    pub activity_type: ActivityType,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    pub price_sats: Option<i64>,
    pub tx_id: Option<String>,
    pub block_height: Option<i64>,
    pub created_at: DateTime<Utc>,
}
