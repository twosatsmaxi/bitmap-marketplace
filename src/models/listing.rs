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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // Mempool protection fields
    pub seller_pubkey: Option<String>,
    pub multisig_address: Option<String>,
    pub multisig_script: Option<String>,
    /// Signed locking raw tx hex — stored but NOT yet broadcast.
    pub locking_raw_tx: Option<String>,
    /// Seller's pre-signed partial_sig (hex) for the sale template multisig input.
    pub seller_sale_sig: Option<String>,
    pub protection_status: String,
    /// Which marketplace this listing was imported from (e.g. "magic_eden"). NULL = native listing.
    pub source_marketplace: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WitnessUtxoRequest {
    pub script_pubkey_hex: String,
    pub value_sats: u64,
}

#[derive(Debug, Deserialize)]
pub struct SpendableInputRequest {
    pub txid: String,
    pub vout: u32,
    pub value_sats: u64,
    pub witness_utxo: WitnessUtxoRequest,
    pub non_witness_utxo_hex: Option<String>,
    pub redeem_script_hex: Option<String>,
    pub witness_script_hex: Option<String>,
    pub sequence: Option<u32>,
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
    pub inscription_input: Option<SpendableInputRequest>,
    /// Optional gas funding UTXO.
    pub gas_funding_input: Option<SpendableInputRequest>,
}

#[derive(Debug, Deserialize)]
pub struct BuyListingRequest {
    pub listing_id: Uuid,
    pub buyer_address: String,
    pub signed_psbt: Option<String>,
    pub buyer_funding_input: Option<SpendableInputRequest>,
    pub fee_rate_sat_vb: Option<f64>,
}
