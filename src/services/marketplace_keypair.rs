use anyhow::{anyhow, Result};
use bitcoin::secp256k1::{All, Message, Secp256k1, SecretKey};
use bitcoin::secp256k1::ecdsa::Signature;
use std::sync::Arc;

pub struct MarketplaceKeypair {
    secp: Secp256k1<All>,
    secret_key: SecretKey,
}

impl MarketplaceKeypair {
    /// Load keypair from MARKETPLACE_SECRET_KEY env var (64-char hex).
    pub fn from_env() -> Result<Arc<Self>> {
        let hex = std::env::var("MARKETPLACE_SECRET_KEY")
            .map_err(|_| anyhow!("MARKETPLACE_SECRET_KEY env var not set"))?;
        let bytes = hex::decode(&hex)
            .map_err(|e| anyhow!("MARKETPLACE_SECRET_KEY is not valid hex: {}", e))?;
        if bytes.len() != 32 {
            return Err(anyhow!(
                "MARKETPLACE_SECRET_KEY must be 64 hex chars (32 bytes), got {} bytes",
                bytes.len()
            ));
        }
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&bytes)
            .map_err(|e| anyhow!("invalid secret key: {}", e))?;
        Ok(Arc::new(Self { secp, secret_key }))
    }

    /// Returns the compressed public key as a 33-byte hex string.
    pub fn pubkey_hex(&self) -> String {
        let pubkey = bitcoin::secp256k1::PublicKey::from_secret_key(&self.secp, &self.secret_key);
        hex::encode(pubkey.serialize())
    }

    /// Returns the raw secp256k1 public key.
    pub fn public_key(&self) -> bitcoin::secp256k1::PublicKey {
        bitcoin::secp256k1::PublicKey::from_secret_key(&self.secp, &self.secret_key)
    }

    /// Signs a 32-byte sighash using ECDSA (SIGHASH_ALL context).
    /// Returns the DER-encoded signature bytes.
    pub fn sign_sighash(&self, sighash: &[u8; 32]) -> Result<Signature> {
        let msg = Message::from_digest(*sighash);
        let sig = self.secp.sign_ecdsa(&msg, &self.secret_key);
        Ok(sig)
    }
}

impl std::fmt::Debug for MarketplaceKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MarketplaceKeypair {{ pubkey: {} }}", self.pubkey_hex())
    }
}
