use anyhow::Result;
use bitcoincore_rpc::{Auth, Client, RpcApi};

pub struct BitcoinRpc {
    client: Client,
}

impl BitcoinRpc {
    pub fn new() -> Result<Self> {
        let url = std::env::var("BITCOIN_RPC_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8332".to_string());
        let user = std::env::var("BITCOIN_RPC_USER")
            .unwrap_or_else(|_| "bitcoin".to_string());
        let pass = std::env::var("BITCOIN_RPC_PASS")
            .unwrap_or_else(|_| "bitcoin".to_string());

        let client = Client::new(&url, Auth::UserPass(user, pass))?;
        Ok(Self { client })
    }

    pub fn get_block_height(&self) -> Result<u64> {
        let count = self.client.get_block_count()?;
        Ok(count)
    }

    pub fn broadcast_transaction(&self, raw_tx: &str) -> Result<String> {
        let txid = self.client.send_raw_transaction(raw_tx)?;
        Ok(txid.to_string())
    }
}
