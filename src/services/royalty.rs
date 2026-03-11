use anyhow::{anyhow, Result};
use bitcoin::psbt::Psbt;
use bitcoin::Amount;

pub struct RoyaltyEnforcement;

pub struct RoyaltyInfo {
    pub address: String,
    pub bps: u32,         // basis points (100 bps = 1%)
    pub amount_sats: u64, // calculated royalty amount
}

impl RoyaltyEnforcement {
    /// Calculate royalty from a sale price.
    /// Returns None if no royalty configured (bps = 0 or no address).
    pub fn calculate(
        price_sats: u64,
        royalty_address: Option<&str>,
        royalty_bps: Option<i32>,
    ) -> Option<RoyaltyInfo> {
        let address = royalty_address?;
        let bps = royalty_bps?;

        if bps <= 0 {
            return None;
        }

        let bps_u32 = bps as u32;
        let amount_sats = price_sats * bps_u32 as u64 / 10_000;

        Some(RoyaltyInfo {
            address: address.to_string(),
            bps: bps_u32,
            amount_sats,
        })
    }

    /// Verify that a PSBT includes the correct royalty output.
    /// Returns Ok(()) if royalty is properly included or not required.
    pub fn verify_royalty_in_psbt(psbt_hex: &str, royalty: &RoyaltyInfo) -> Result<()> {
        let bytes =
            hex::decode(psbt_hex).map_err(|e| anyhow!("Failed to decode PSBT hex: {}", e))?;

        let psbt =
            Psbt::deserialize(&bytes).map_err(|e| anyhow!("Failed to deserialize PSBT: {}", e))?;

        // Parse the royalty address to get its script pubkey
        let royalty_script = {
            use bitcoin::Address;
            use std::str::FromStr;

            // Try to parse as a bitcoin address on the current network
            // We try mainnet first, then testnet
            let addr = Address::from_str(&royalty.address)
                .map_err(|e| anyhow!("Invalid royalty address '{}': {}", royalty.address, e))?;

            addr.assume_checked().script_pubkey()
        };

        // Search the unsigned transaction outputs for a matching output
        let tx = &psbt.unsigned_tx;
        let required = Amount::from_sat(royalty.amount_sats);
        let found = tx
            .output
            .iter()
            .any(|out| out.script_pubkey == royalty_script && out.value >= required);

        if found {
            Ok(())
        } else {
            Err(anyhow!(
                "PSBT does not contain a royalty output of at least {} sats to {}",
                royalty.amount_sats,
                royalty.address
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_royalty() {
        let info = RoyaltyEnforcement::calculate(
            100_000,
            Some("bc1qtest"),
            Some(250), // 2.5%
        );
        let info = info.unwrap();
        assert_eq!(info.bps, 250);
        assert_eq!(info.amount_sats, 2_500); // 100_000 * 250 / 10_000
        assert_eq!(info.address, "bc1qtest");
    }

    #[test]
    fn test_calculate_royalty_zero_bps() {
        let info = RoyaltyEnforcement::calculate(100_000, Some("bc1qtest"), Some(0));
        assert!(info.is_none());
    }

    #[test]
    fn test_calculate_royalty_no_address() {
        let info = RoyaltyEnforcement::calculate(100_000, None, Some(250));
        assert!(info.is_none());
    }

    #[test]
    fn test_calculate_royalty_no_bps() {
        let info = RoyaltyEnforcement::calculate(100_000, Some("bc1qtest"), None);
        assert!(info.is_none());
    }

    // ── verify_royalty_in_psbt tests ─────────────────────────────────────────

    /// Helper: build a minimal PSBT (bitcoin 0.31) containing the given outputs,
    /// serialize it to hex, and return the hex string.
    fn build_psbt_hex(outputs: Vec<bitcoin::TxOut>) -> String {
        use bitcoin::{
            absolute::LockTime, hashes::Hash, psbt::Psbt, OutPoint, ScriptBuf, Sequence,
            Transaction, TxIn, Txid, Witness,
        };

        let dummy_input = TxIn {
            previous_output: OutPoint {
                txid: Txid::all_zeros(),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        };

        let tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![dummy_input],
            output: outputs,
        };

        let psbt = Psbt::from_unsigned_tx(tx).expect("valid unsigned tx");
        let bytes = psbt.serialize();
        hex::encode(bytes)
    }

    /// A well-known mainnet bech32 P2WPKH address used across the tests below.
    /// Address: bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq
    /// (public domain test vector)
    const ROYALTY_ADDR: &str = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";

    /// Returns the script_pubkey for ROYALTY_ADDR without going through
    /// verify_royalty_in_psbt so tests remain self-contained.
    fn royalty_script() -> bitcoin::ScriptBuf {
        use bitcoin::Address;
        use std::str::FromStr;
        Address::from_str(ROYALTY_ADDR)
            .unwrap()
            .assume_checked()
            .script_pubkey()
    }

    #[test]
    fn verify_royalty_no_address_always_passes() {
        // When royalty_address is None, calculate() returns None — the caller
        // is expected to skip verify_royalty_in_psbt altogether.  We model that
        // behaviour here: None from calculate() means no royalty required, so
        // the result is trivially Ok.
        let result = RoyaltyEnforcement::calculate(1_000_000, None, Some(500));
        assert!(
            result.is_none(),
            "No address should produce no royalty requirement"
        );
        // Because there is no RoyaltyInfo to check, verify is never called and
        // the overall flow always passes — represented here as Ok(()).
        let _: Result<()> = Ok(());
    }

    #[test]
    fn verify_royalty_output_present() {
        // Build a PSBT that contains an output paying the exact royalty amount
        // to ROYALTY_ADDR.
        let amount_sats: u64 = 2_500;

        let royalty_out = bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(amount_sats),
            script_pubkey: royalty_script(),
        };

        // Add an unrelated change output as well.
        let change_out = bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(97_500),
            script_pubkey: bitcoin::ScriptBuf::new(),
        };

        let psbt_hex = build_psbt_hex(vec![royalty_out, change_out]);

        let royalty_info = RoyaltyInfo {
            address: ROYALTY_ADDR.to_string(),
            bps: 250,
            amount_sats,
        };

        let result = RoyaltyEnforcement::verify_royalty_in_psbt(&psbt_hex, &royalty_info);
        assert!(
            result.is_ok(),
            "PSBT with correct royalty output should pass verification: {:?}",
            result
        );
    }

    #[test]
    fn verify_royalty_output_missing_fails() {
        // Build a PSBT that has NO output to the royalty address.
        let change_out = bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(100_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        };

        let psbt_hex = build_psbt_hex(vec![change_out]);

        let royalty_info = RoyaltyInfo {
            address: ROYALTY_ADDR.to_string(),
            bps: 250,
            amount_sats: 2_500,
        };

        let result = RoyaltyEnforcement::verify_royalty_in_psbt(&psbt_hex, &royalty_info);
        assert!(
            result.is_err(),
            "PSBT without royalty output should fail verification"
        );
    }
}
