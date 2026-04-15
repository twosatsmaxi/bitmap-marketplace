// Re-export modules for integration tests.
// The binary crate (main.rs) handles startup; this lib crate exposes internals.

pub mod db;
pub mod errors;
pub mod models;
pub mod routes;
pub mod services;
pub mod ws;

use std::sync::Arc;

use bitcoin::Network;
use secrecy::SecretString;

use crate::db::Database;
use crate::services::marketplace_keypair::MarketplaceKeypair;
use crate::services::ord::OrdClient;

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub ws_broadcaster: Arc<ws::WsBroadcaster>,
    pub marketplace_keypair: Arc<MarketplaceKeypair>,
    pub http_client: reqwest::Client,
    pub ord_client: OrdClient,
    pub render_api_base: String,
    pub network: Network,
    pub allowed_address_network: Network,
    pub jwt_secret: SecretString,
    pub marketplace_fee_address: Option<String>,
    pub marketplace_fee_bps: u64,
    pub challenges: moka::future::Cache<String, routes::auth::Challenge>,
}
