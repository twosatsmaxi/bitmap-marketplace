use anyhow::{anyhow, Result};
use bitcoin::address::NetworkUnchecked;
use bitcoin::consensus::deserialize;
use bitcoin::{Network, Transaction, Txid};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::str::FromStr;

pub struct BitcoinRpc {
    client: Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoInfo {
    pub txid: String,
    pub vout: u32,
    pub amount_sats: u64,
    pub confirmations: u32,
    pub address: Option<String>,
    pub script_pubkey: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeEstimate {
    pub fee_rate_sat_vb: f64,
    pub blocks_target: u32,
}

impl BitcoinRpc {
    /// Creates a new BitcoinRpc client by reading connection parameters from environment variables:
    /// - `BITCOIN_RPC_URL` (default: `http://127.0.0.1:8332`)
    /// - `BITCOIN_RPC_USER` (default: `bitcoin`)
    /// - `BITCOIN_RPC_PASS` (default: `bitcoin`)
    pub fn new() -> Result<Self> {
        let url = std::env::var("BITCOIN_RPC_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8332".to_string());
        let user = std::env::var("BITCOIN_RPC_USER").unwrap_or_else(|_| "bitcoin".to_string());
        let pass = std::env::var("BITCOIN_RPC_PASS").unwrap_or_else(|_| "bitcoin".to_string());

        let client = Client::new(&url, Auth::UserPass(user, pass))?;
        Ok(Self { client })
    }

    /// Returns the current best block height (block count).
    pub fn get_block_count(&self) -> Result<u64> {
        let count = self.client.get_block_count()?;
        Ok(count)
    }

    /// Fetches and deserializes a raw `Transaction` by its txid string.
    pub fn get_raw_transaction(&self, txid: &str) -> Result<Transaction> {
        let txid_obj = Txid::from_str(txid)?;
        let tx = self.client.get_raw_transaction(&txid_obj, None)?;
        Ok(tx)
    }

    /// Decodes a hex-encoded raw transaction, broadcasts it to the network,
    /// and returns the resulting txid as a string.
    pub fn broadcast_transaction(&self, raw_tx_hex: &str) -> Result<String> {
        // Decode hex into bytes and deserialize to verify the transaction is valid.
        let raw_bytes = hex::decode(raw_tx_hex)?;
        let _tx: Transaction = deserialize(&raw_bytes)?;

        // Send the raw hex string; the RPC client accepts anything implementing RawTx.
        let txid = self.client.send_raw_transaction(raw_tx_hex)?;
        Ok(txid.to_string())
    }

    /// Returns UTXO information for the given outpoint, or `None` if the output
    /// is spent or does not exist (uses `gettxout` RPC).
    pub fn get_utxo_info(&self, txid: &str, vout: u32) -> Result<Option<UtxoInfo>> {
        let txid_obj = Txid::from_str(txid)?;

        let result = self.client.get_tx_out(&txid_obj, vout, Some(true))?;

        match result {
            None => Ok(None),
            Some(out) => {
                let amount_sats = out.value.to_sat();

                // Prefer the modern `address` field; fall back to the first entry in
                // the deprecated `addresses` vec (Bitcoin Core < 22).
                let address = out
                    .script_pub_key
                    .address
                    .as_ref()
                    .or_else(|| out.script_pub_key.addresses.first())
                    .map(|a: &bitcoin::Address<NetworkUnchecked>| {
                        a.clone().assume_checked_ref().to_string()
                    });

                let script_pubkey = hex::encode(&out.script_pub_key.hex);

                Ok(Some(UtxoInfo {
                    txid: txid.to_string(),
                    vout,
                    amount_sats,
                    confirmations: out.confirmations,
                    address,
                    script_pubkey,
                }))
            }
        }
    }

    /// Estimates the fee rate for confirmation within `target_blocks` blocks.
    /// Converts the `estimatesmartfee` result from BTC/kvB to sat/vB.
    pub fn estimate_fee_rate(&self, target_blocks: u16) -> Result<FeeEstimate> {
        let result = self.client.estimate_smart_fee(target_blocks, None)?;

        let fee_rate = result.fee_rate.ok_or_else(|| {
            anyhow!("Fee estimation unavailable: no fee rate returned (need more data)")
        })?;

        // fee_rate is in BTC/kvB; convert to sat/vB:
        //   1 BTC = 100_000_000 sat, 1 kvB = 1000 vB
        //   sat/vB = BTC/kvB * 100_000_000 / 1000 = BTC/kvB * 100_000
        let btc_per_kvb = fee_rate.to_btc();
        let fee_rate_sat_vb = btc_per_kvb * 100_000.0;

        Ok(FeeEstimate {
            fee_rate_sat_vb,
            blocks_target: result.blocks as u32,
        })
    }

    /// Validates a Bitcoin address string using the `getaddressinfo` RPC.
    /// Returns `true` if the address is syntactically valid and recognized by the node.
    pub fn validate_address(&self, address: &str) -> Result<bool> {
        // Parse into an unchecked Address, then assume_checked for the RPC call.
        // If parsing fails outright the address is definitely invalid.
        let parsed = address.parse::<bitcoin::Address<NetworkUnchecked>>();
        let addr = match parsed {
            Ok(a) => a.assume_checked(),
            Err(_) => return Ok(false),
        };

        // `getaddressinfo` is available on wallet-enabled nodes and will error if the
        // address is not recognized. We treat any RPC error as "invalid".
        match self.client.get_address_info(&addr) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Returns the `Network` the connected node is running on by inspecting
    /// the `chain` field of `getblockchaininfo`.
    pub fn get_network(&self) -> Result<Network> {
        let info = self.client.get_blockchain_info()?;
        // The bitcoincore-rpc-json crate deserializes `chain` directly into `Network`.
        Ok(info.chain)
    }

    /// Submit a package of transactions atomically via `submitpackage` RPC (Bitcoin Core v25+).
    /// `txns` is a slice of raw transaction hex strings; the first should be the parent (locking tx),
    /// the second the child (sale tx).
    /// Returns the txids of successfully accepted transactions.
    pub fn submit_package(&self, txns: &[&str]) -> Result<Vec<String>> {
        let tx_array: Vec<serde_json::Value> = txns.iter().map(|tx| json!(tx)).collect();
        let result: serde_json::Value = self.client.call("submitpackage", &[json!(tx_array)])?;

        // submitpackage returns {"tx-results": {txid: {...}}, "replaced-transactions": [...]}
        // Extract accepted txids from tx-results keys.
        let txids = result
            .get("tx-results")
            .and_then(|r| r.as_object())
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        Ok(txids)
    }
}
