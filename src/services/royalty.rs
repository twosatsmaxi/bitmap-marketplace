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
        let bytes = hex::decode(psbt_hex)
            .map_err(|e| anyhow!("Failed to decode PSBT hex: {}", e))?;

        let psbt = Psbt::deserialize(&bytes)
            .map_err(|e| anyhow!("Failed to deserialize PSBT: {}", e))?;

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
        let found = tx.output.iter().any(|out| {
            out.script_pubkey == royalty_script && out.value >= required
        });

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
}
