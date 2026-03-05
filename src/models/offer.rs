use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "offer_status", rename_all = "lowercase")]
pub enum OfferStatus {
    Pending,
    Accepted,
    Rejected,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Offer {
    pub id: Uuid,
    pub inscription_id: String,
    pub buyer_address: String,
    pub price_sats: i64,
    pub status: OfferStatus,
    pub psbt: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
