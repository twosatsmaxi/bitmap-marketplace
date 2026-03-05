// PSBT construction for trustless listing and buying.
// Full implementation lives in bitmap-marketplace-ssd (listing) and bitmap-marketplace-jaa (buy).

use anyhow::Result;
use bitcoin::psbt::Psbt;
use std::str::FromStr;

pub fn decode_psbt(hex: &str) -> Result<Psbt> {
    let bytes = hex::decode(hex).map_err(|e| anyhow::anyhow!("invalid hex: {}", e))?;
    let psbt = Psbt::deserialize(&bytes)?;
    Ok(psbt)
}

pub fn encode_psbt(psbt: &Psbt) -> String {
    hex::encode(psbt.serialize())
}
