use anyhow::{anyhow, Result};
use bitcoin::secp256k1::ecdsa::Signature;
use bitcoin::secp256k1::{All, Message, Secp256k1, SecretKey};
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
        let secret_key =
            SecretKey::from_slice(&bytes).map_err(|e| anyhow!("invalid secret key: {}", e))?;
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

/// Test-only constructor — produces a valid but insecure keypair from a known seed.
#[cfg(test)]
impl MarketplaceKeypair {
    pub fn for_testing() -> Arc<Self> {
        // 0x01 key: well-known test seed, valid for secp256k1 (far below curve order)
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[0x01u8; 32]).unwrap();
        Arc::new(Self { secp, secret_key })
    }
}

impl std::fmt::Debug for MarketplaceKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MarketplaceKeypair {{ pubkey: {} }}", self.pubkey_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn env_lock() -> &'static Mutex<()> {
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    // A valid 32-byte secret key in hex (not zero, not above curve order)
    const VALID_KEY_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    #[test]
    fn from_env_with_valid_key() {
        let _lock = env_lock().lock().unwrap();
        std::env::set_var("MARKETPLACE_SECRET_KEY", VALID_KEY_HEX);
        let kp = MarketplaceKeypair::from_env().unwrap();
        // Compressed pubkey is 33 bytes = 66 hex chars
        assert_eq!(kp.pubkey_hex().len(), 66);
        std::env::remove_var("MARKETPLACE_SECRET_KEY");
    }

    #[test]
    fn from_env_missing_var() {
        let _lock = env_lock().lock().unwrap();
        std::env::remove_var("MARKETPLACE_SECRET_KEY");
        let err = MarketplaceKeypair::from_env().unwrap_err();
        assert!(err.to_string().contains("not set"));
    }

    #[test]
    fn from_env_invalid_hex() {
        let _lock = env_lock().lock().unwrap();
        std::env::set_var("MARKETPLACE_SECRET_KEY", "not-hex-at-all!!");
        let err = MarketplaceKeypair::from_env().unwrap_err();
        assert!(err.to_string().contains("not valid hex"));
        std::env::remove_var("MARKETPLACE_SECRET_KEY");
    }

    #[test]
    fn from_env_wrong_length() {
        let _lock = env_lock().lock().unwrap();
        std::env::set_var("MARKETPLACE_SECRET_KEY", "aabbccdd"); // only 4 bytes
        let err = MarketplaceKeypair::from_env().unwrap_err();
        assert!(err.to_string().contains("32 bytes"));
        std::env::remove_var("MARKETPLACE_SECRET_KEY");
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let _lock = env_lock().lock().unwrap();
        std::env::set_var("MARKETPLACE_SECRET_KEY", VALID_KEY_HEX);
        let kp = MarketplaceKeypair::from_env().unwrap();
        std::env::remove_var("MARKETPLACE_SECRET_KEY");

        let sighash = [0x42u8; 32];
        let sig = kp.sign_sighash(&sighash).unwrap();

        // Verify the signature with the public key
        let secp = Secp256k1::verification_only();
        let msg = Message::from_digest(sighash);
        secp.verify_ecdsa(&msg, &sig, &kp.public_key()).unwrap();
    }

    #[test]
    fn pubkey_is_deterministic() {
        let _lock = env_lock().lock().unwrap();
        std::env::set_var("MARKETPLACE_SECRET_KEY", VALID_KEY_HEX);
        let kp1 = MarketplaceKeypair::from_env().unwrap();
        let kp2 = MarketplaceKeypair::from_env().unwrap();
        std::env::remove_var("MARKETPLACE_SECRET_KEY");

        assert_eq!(kp1.pubkey_hex(), kp2.pubkey_hex());
    }
}
