use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "listing_status", rename_all = "lowercase")]
pub enum ListingStatus {
    Active,
    Sold,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Listing {
    pub id: Uuid,
    pub inscription_id: String,
    pub seller_address: String,
    pub price_sats: i64,
    pub status: ListingStatus,
    /// Partially-signed Bitcoin transaction (hex-encoded)
    pub psbt: Option<String>,
    pub royalty_address: Option<String>,
    pub royalty_bps: Option<i32>,  // basis points
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateListingRequest {
    pub inscription_id: String,
    pub price_sats: i64,
    pub seller_address: String,
    pub unsigned_psbt: String,
}

#[derive(Debug, Deserialize)]
pub struct BuyListingRequest {
    pub listing_id: Uuid,
    pub buyer_address: String,
    pub signed_psbt: String,
}
