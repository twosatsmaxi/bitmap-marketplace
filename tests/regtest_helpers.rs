// Helpers for regtest integration tests.
// Manages testcontainers (Postgres + bitcoind) and provides RPC + app utilities.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use bitcoin::Network;
use bitcoincore_rpc::{Auth, Client, RpcApi};
use secrecy::SecretString;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::core::WaitFor;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use testcontainers_modules::postgres::Postgres;

use bitmap_marketplace::db::Database;
use bitmap_marketplace::services::marketplace_keypair::MarketplaceKeypair;
use bitmap_marketplace::services::ord::OrdClient;
use bitmap_marketplace::ws::WsBroadcaster;
use bitmap_marketplace::AppState;

/// Known test secret key (32 bytes of 0x01) — matches MarketplaceKeypair::for_testing().
pub const TEST_MARKETPLACE_SECRET_HEX: &str =
    "0101010101010101010101010101010101010101010101010101010101010101";

/// All test infrastructure: containers + connection details.
pub struct TestInfra {
    pub pg_container: ContainerAsync<Postgres>,
    pub btc_container: ContainerAsync<GenericImage>,
    pub db_url: String,
    pub rpc_url: String,
}

impl TestInfra {
    /// Start Postgres + bitcoind containers and return connection info.
    pub async fn start() -> Self {
        // Start Postgres
        // Use Postgres 16: gen_random_uuid() is built-in since v13, and the
        // testcontainers default (11-alpine) doesn't have it.
        let pg_container = Postgres::default()
            .with_tag("16-alpine")
            .start()
            .await
            .expect("failed to start postgres container");
        let pg_port = pg_container.get_host_port_ipv4(5432).await.unwrap();
        let db_url = format!("postgres://postgres:postgres@127.0.0.1:{pg_port}/postgres");

        // Start bitcoind in regtest mode
        // Uses the official bitcoin/bitcoin image (v28+, supports submitpackage).
        // Pass args with leading `-` so the entrypoint prepends `bitcoind` automatically.
        let btc_container = GenericImage::new("bitcoin/bitcoin", "28.1")
            .with_exposed_port(18443.into())
            .with_wait_for(WaitFor::message_on_stdout("Done loading"))
            .with_startup_timeout(Duration::from_secs(60))
            .with_cmd(vec![
                "-regtest".to_string(),
                "-server".to_string(),
                "-txindex".to_string(),
                "-printtoconsole".to_string(),
                "-rpcport=18443".to_string(),
                "-rpcuser=bitcoin".to_string(),
                "-rpcpassword=bitcoin".to_string(),
                "-rpcallowip=0.0.0.0/0".to_string(),
                "-rpcbind=0.0.0.0".to_string(),
                "-fallbackfee=0.00001".to_string(),
            ])
            .start()
            .await
            .expect("failed to start bitcoind container");

        // Use 127.0.0.1 explicitly: get_host() returns "localhost" which resolves
        // to IPv6 (::1) first on macOS, but Docker binds ports to 0.0.0.0 (IPv4 only).
        let btc_port = btc_container.get_host_port_ipv4(18443).await.unwrap();
        let rpc_url = format!("http://127.0.0.1:{btc_port}");
        eprintln!("[regtest] bitcoind RPC URL: {rpc_url}");

        // Wait for bitcoind to be ready
        let rpc = Self::wait_for_rpc(&rpc_url).await;

        // Create default wallet and mine 101 blocks for coinbase maturity
        rpc.create_wallet("test_wallet", None, None, None, None)
            .expect("failed to create wallet");
        let addr = rpc
            .get_new_address(None, Some(bitcoincore_rpc::json::AddressType::Bech32))
            .unwrap()
            .assume_checked();
        rpc.generate_to_address(101, &addr)
            .expect("failed to mine initial blocks");

        Self {
            pg_container,
            btc_container,
            db_url,
            rpc_url,
        }
    }

    async fn wait_for_rpc(url: &str) -> Client {
        let mut last_err = String::new();
        for _ in 0..60 {
            match Client::new(url, Auth::UserPass("bitcoin".into(), "bitcoin".into())) {
                // Use get_block_count() instead of get_blockchain_info() because
                // bitcoincore_rpc 0.18 can't deserialize Bitcoin Core v28's response
                // (warnings field changed from String to Vec<String>).
                Ok(client) => match client.get_block_count() {
                    Ok(_) => return client,
                    Err(e) => last_err = format!("RPC call failed: {e}"),
                },
                Err(e) => last_err = format!("Client creation failed: {e}"),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("bitcoind did not become ready within 30 seconds. Last error: {last_err}");
    }
}

/// RPC helper for regtest operations.
pub struct RpcHelper {
    client: Client,
}

impl RpcHelper {
    pub fn new(rpc_url: &str) -> Self {
        let client = Client::new(rpc_url, Auth::UserPass("bitcoin".into(), "bitcoin".into()))
            .expect("failed to create RPC client");
        Self { client }
    }

    /// Get a new bech32 (P2WPKH) address from the test wallet.
    pub fn new_address(&self) -> bitcoin::Address {
        self.client
            .get_new_address(None, Some(bitcoincore_rpc::json::AddressType::Bech32))
            .unwrap()
            .assume_checked()
    }

    /// Send `amount` BTC to `addr` and mine 1 block to confirm it.
    /// Returns the funding txid.
    pub fn fund_address(&self, addr: &bitcoin::Address, amount_btc: f64) -> bitcoin::Txid {
        let txid = self
            .client
            .send_to_address(
                addr,
                bitcoin::Amount::from_btc(amount_btc).unwrap(),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("failed to fund address");
        self.mine_blocks(1);
        txid
    }

    /// Mine `n` blocks.
    pub fn mine_blocks(&self, n: u64) {
        let addr = self.new_address();
        self.client
            .generate_to_address(n, &addr)
            .expect("failed to mine blocks");
    }

    /// Get the raw transaction.
    pub fn get_raw_transaction(&self, txid: &bitcoin::Txid) -> bitcoin::Transaction {
        self.client
            .get_raw_transaction(txid, None)
            .expect("failed to get raw tx")
    }

    /// Get UTXO info (script_pubkey_hex, value_sats) for a specific outpoint.
    pub fn get_utxo_info(&self, txid: &bitcoin::Txid, vout: u32) -> (String, u64) {
        let tx = self.get_raw_transaction(txid);
        let output = &tx.output[vout as usize];
        (
            hex::encode(output.script_pubkey.as_bytes()),
            output.value.to_sat(),
        )
    }

    /// Sign a PSBT using the wallet's keys. Returns the signed PSBT base64.
    /// Bitcoin Core v28 rejects sighash mismatches, so callers must pass the
    /// correct sighash type matching what's stored in the PSBT inputs.
    pub fn wallet_sign_psbt(&self, psbt_base64: &str) -> String {
        self.wallet_sign_psbt_with_sighash(psbt_base64, "ALL")
    }

    /// Sign a PSBT with a specific sighash type.
    pub fn wallet_sign_psbt_with_sighash(&self, psbt_base64: &str, sighash: &str) -> String {
        let result: serde_json::Value = self
            .client
            .call(
                "walletprocesspsbt",
                &[
                    serde_json::json!(psbt_base64),
                    serde_json::json!(true),
                    serde_json::json!(sighash),
                ],
            )
            .expect("walletprocesspsbt failed");
        result["psbt"].as_str().unwrap().to_string()
    }

    /// Get the underlying RPC client.
    pub fn client(&self) -> &Client {
        &self.client
    }
}

/// Build a test AppState connected to the test infrastructure.
pub async fn build_test_state(infra: &TestInfra) -> AppState {
    // Set env vars for BitcoinRpc::new() (used by confirm_order route)
    std::env::set_var("BITCOIN_RPC_URL", &infra.rpc_url);
    std::env::set_var("BITCOIN_RPC_USER", "bitcoin");
    std::env::set_var("BITCOIN_RPC_PASS", "bitcoin");
    std::env::set_var("MARKETPLACE_SECRET_KEY", TEST_MARKETPLACE_SECRET_HEX);

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&infra.db_url)
        .await
        .expect("failed to connect to test postgres");

    let db = Database { pool };
    db.run_migrations()
        .await
        .expect("failed to run migrations");

    let marketplace_keypair =
        MarketplaceKeypair::from_env().expect("failed to create marketplace keypair");

    AppState {
        db,
        ws_broadcaster: Arc::new(WsBroadcaster::new()),
        marketplace_keypair,
        http_client: reqwest::Client::new(),
        ord_client: OrdClient::new(),
        render_api_base: "http://localhost:9999".to_string(),
        network: Network::Regtest,
        allowed_address_network: Network::Regtest,
        jwt_secret: SecretString::new("test-jwt-secret-for-integration-tests".to_string()),
        marketplace_fee_address: None,
        marketplace_fee_bps: 0,
        challenges: moka::future::Cache::builder().max_capacity(100).build(),
    }
}

/// Build the full Axum router with test state (matches main.rs routing).
pub fn build_test_router(state: AppState) -> axum::Router {
    use axum::routing::get;
    use axum::Router;

    Router::new()
        .nest("/api", bitmap_marketplace::routes::router())
        .route(
            "/health",
            get(|axum::extract::State(s): axum::extract::State<AppState>| async move {
                match sqlx::query("SELECT 1").execute(&s.db.pool).await {
                    Ok(_) => axum::http::StatusCode::OK,
                    Err(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
                }
            }),
        )
        .with_state(state)
}

/// Truncate all test tables for isolation between tests.
pub async fn cleanup_db(state: &AppState) {
    sqlx::query("TRUNCATE sales, offers, activity_feed, listings, inscriptions, collections, profiles CASCADE")
        .execute(&state.db.pool)
        .await
        .expect("failed to truncate tables");
}

/// Convert a PSBT from hex encoding to base64 (for walletprocesspsbt).
pub fn psbt_hex_to_base64(hex_str: &str) -> String {
    use base64::Engine;
    let bytes = hex::decode(hex_str).expect("invalid hex");
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

/// Convert a PSBT from base64 to hex encoding.
pub fn psbt_base64_to_hex(b64: &str) -> String {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("invalid base64");
    hex::encode(bytes)
}
