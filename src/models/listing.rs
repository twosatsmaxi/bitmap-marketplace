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
    pub royalty_bps: Option<i32>, // basis points
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // Mempool protection fields
    pub seller_pubkey: Option<String>,
    pub multisig_address: Option<String>,
    pub multisig_script: Option<String>,
    /// Signed locking raw tx hex — stored but NOT yet broadcast.
    pub locking_raw_tx: Option<String>,
    pub protection_status: String,
    /// Which marketplace this listing was imported from (e.g. "magic_eden"). NULL = native listing.
    pub source_marketplace: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateListingRequest {
    pub inscription_id: String,
    pub price_sats: i64,
    pub seller_address: String,
    pub unsigned_psbt: Option<String>,
    /// Optional compressed secp256k1 pubkey (hex) to enable mempool protection.
    pub seller_pubkey: Option<String>,
    /// Required when seller_pubkey is set.
    pub inscription_txid: Option<String>,
    pub inscription_vout: Option<u32>,
    /// Inscription UTXO value in sats (for fee calculation).
    pub inscription_amount_sats: Option<u64>,
    /// Optional gas funding UTXO fields.
    pub gas_txid: Option<String>,
    pub gas_vout: Option<u32>,
    pub gas_amount_sats: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct BuyListingRequest {
    pub listing_id: Uuid,
    pub buyer_address: String,
    pub signed_psbt: Option<String>,
    pub buyer_utxo_txid: Option<String>,
    pub buyer_utxo_vout: Option<u32>,
    pub buyer_utxo_amount_sats: Option<u64>,
    pub fee_rate_sat_vb: Option<f64>,
}
