// PSBT construction for trustless listing and buying.
// bitmap-marketplace-ssd: listing construction
// bitmap-marketplace-jaa: buy flow completion

use anyhow::{anyhow, Result};
use bitcoin::{
    absolute::LockTime,
    psbt::{Input as PsbtInput, Output as PsbtOutput, Psbt},
    Address, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

pub struct ListingRequest {
    pub inscription_txid: String,
    pub inscription_vout: u32,
    pub seller_address: String, // base58/bech32 Bitcoin address
    pub price_sats: u64,
}

pub struct ListingPsbt {
    pub psbt_hex: String, // unsigned PSBT for seller to sign
    pub inscription_txid: String,
    pub inscription_vout: u32,
}

pub struct BuyRequest {
    pub seller_psbt_hex: String, // seller's signed PSBT from DB
    pub buyer_address: String,
    pub buyer_utxo_txid: String,
    pub buyer_utxo_vout: u32,
    pub buyer_utxo_amount_sats: u64,
    pub fee_rate_sat_vb: f64,
}

pub struct BuyPsbt {
    pub psbt_hex: String, // combined PSBT for buyer to sign
    pub estimated_fee_sats: u64,
}

// ---------------------------------------------------------------------------
// Listing construction (bitmap-marketplace-ssd)
// ---------------------------------------------------------------------------

/// Build an unsigned PSBT that the seller signs with SIGHASH_SINGLE | ANYONECANPAY.
///
/// The resulting PSBT has:
///   - Input  0: inscription UTXO (seller provides, signed with SINGLE|ACP)
///   - Output 0: payment to seller for `price_sats`
///
/// The buyer will later add their own inputs/outputs before broadcasting.
pub fn build_listing_psbt(req: &ListingRequest) -> Result<ListingPsbt> {
    // Parse the inscription outpoint.
    let txid = Txid::from_str(&req.inscription_txid)
        .map_err(|e| anyhow!("invalid inscription txid: {}", e))?;
    let outpoint = OutPoint::new(txid, req.inscription_vout);

    // Parse seller address → script_pubkey.
    let seller_addr = Address::from_str(&req.seller_address)
        .map_err(|e| anyhow!("invalid seller address: {}", e))?
        .assume_checked();
    let seller_script = seller_addr.script_pubkey();

    // Build the unsigned transaction skeleton.
    let tx_in = TxIn {
        previous_output: outpoint,
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::default(),
    };

    let tx_out = TxOut {
        value: Amount::from_sat(req.price_sats),
        script_pubkey: seller_script,
    };

    let unsigned_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![tx_in],
        output: vec![tx_out],
    };

    // Wrap in a PSBT.
    let mut psbt =
        Psbt::from_unsigned_tx(unsigned_tx).map_err(|e| anyhow!("PSBT creation failed: {}", e))?;

    // Annotate input 0 with SIGHASH_SINGLE | ANYONECANPAY so the seller's wallet
    // knows which sighash type to use when signing.
    psbt.inputs[0].sighash_type = Some(bitcoin::psbt::PsbtSighashType::from(
        bitcoin::EcdsaSighashType::SinglePlusAnyoneCanPay,
    ));

    Ok(ListingPsbt {
        psbt_hex: encode_psbt(&psbt),
        inscription_txid: req.inscription_txid.clone(),
        inscription_vout: req.inscription_vout,
    })
}

// ---------------------------------------------------------------------------
// Buy flow completion (bitmap-marketplace-jaa)
// ---------------------------------------------------------------------------

/// Estimate transaction size in vbytes using the formula:
///   10 + 41 * n_inputs + 31 * n_outputs
fn estimate_tx_vbytes(n_inputs: usize, n_outputs: usize) -> u64 {
    (10 + 41 * n_inputs + 31 * n_outputs) as u64
}

/// Add the buyer's inputs/outputs to the seller's signed PSBT and return the
/// combined PSBT hex for the buyer to sign their own input.
///
/// The final transaction will have:
///   - Input  0: inscription UTXO (seller, already signed with SINGLE|ACP)
///   - Input  1: buyer's payment UTXO
///   - Output 0: payment to seller (carried from seller PSBT)
///   - Output 1: inscription dust to buyer (546 sats)
///   - Output 2: change back to buyer
pub fn build_buy_psbt(req: &BuyRequest) -> Result<BuyPsbt> {
    // Deserialise the seller's PSBT.
    let psbt = decode_psbt(&req.seller_psbt_hex)?;

    // Validate basic structure: must have exactly 1 input and 1 output (the seller's).
    if psbt.unsigned_tx.input.len() != 1 || psbt.unsigned_tx.output.len() != 1 {
        return Err(anyhow!(
            "seller PSBT has unexpected structure: expected 1 input and 1 output"
        ));
    }

    // Price is what the seller expects (output 0 value).
    let price_sats = psbt.unsigned_tx.output[0].value.to_sat();

    // Dust limit for the inscription output delivered to buyer (P2WPKH minimum).
    const INSCRIPTION_DUST_SATS: u64 = 546;

    // Parse the buyer's address.
    let buyer_addr = Address::from_str(&req.buyer_address)
        .map_err(|e| anyhow!("invalid buyer address: {}", e))?
        .assume_checked();
    let buyer_script = buyer_addr.script_pubkey();

    // Estimate fee (3 inputs would be worst case; we have 2 inputs, 3 outputs).
    let n_inputs = 2usize;
    let n_outputs = 3usize;
    let estimated_fee_sats =
        (estimate_tx_vbytes(n_inputs, n_outputs) as f64 * req.fee_rate_sat_vb) as u64;

    // Verify the buyer's UTXO covers price + dust + fee.
    let total_required = price_sats + INSCRIPTION_DUST_SATS + estimated_fee_sats;
    if req.buyer_utxo_amount_sats < total_required {
        return Err(anyhow!(
            "buyer UTXO amount ({} sats) is insufficient; need at least {} sats (price {} + dust {} + fee {})",
            req.buyer_utxo_amount_sats,
            total_required,
            price_sats,
            INSCRIPTION_DUST_SATS,
            estimated_fee_sats
        ));
    }

    let change_sats =
        req.buyer_utxo_amount_sats - price_sats - INSCRIPTION_DUST_SATS - estimated_fee_sats;

    // Build the additional input (buyer's UTXO).
    let buyer_txid = Txid::from_str(&req.buyer_utxo_txid)
        .map_err(|e| anyhow!("invalid buyer UTXO txid: {}", e))?;
    let buyer_outpoint = OutPoint::new(buyer_txid, req.buyer_utxo_vout);
    let buyer_tx_in = TxIn {
        previous_output: buyer_outpoint,
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::default(),
    };

    // Output 1: inscription dust to buyer.
    let inscription_out = TxOut {
        value: Amount::from_sat(INSCRIPTION_DUST_SATS),
        script_pubkey: buyer_script.clone(),
    };

    // Output 2: change back to buyer.
    let change_out = TxOut {
        value: Amount::from_sat(change_sats),
        script_pubkey: buyer_script,
    };

    // Reconstruct the full unsigned transaction by cloning the seller's skeleton
    // and appending the buyer's input and outputs.
    let mut tx = psbt.unsigned_tx.clone();
    tx.input.push(buyer_tx_in);
    tx.output.push(inscription_out);
    tx.output.push(change_out);

    // Rebuild the PSBT from the extended transaction.
    // We need to carry over the seller's input metadata (sighash type, witness UTXO, partial sigs).
    let seller_input_meta = psbt.inputs[0].clone();
    let seller_output_meta = psbt.outputs[0].clone();

    let mut new_psbt = Psbt::from_unsigned_tx(tx)
        .map_err(|e| anyhow!("PSBT creation from extended tx failed: {}", e))?;

    // Restore seller's input metadata (including partial_sigs already present).
    new_psbt.inputs[0] = seller_input_meta;

    // Restore seller's output metadata.
    new_psbt.outputs[0] = seller_output_meta;

    // Annotate buyer's input (index 1) with the buyer's UTXO info.
    // We set witness_utxo so signing libraries can compute the sighash.
    new_psbt.inputs[1] = PsbtInput {
        witness_utxo: Some(TxOut {
            value: Amount::from_sat(req.buyer_utxo_amount_sats),
            // We don't know the exact script_pubkey without a lookup, so leave it
            // empty here; the buyer's wallet will fill it in when signing.
            script_pubkey: ScriptBuf::new(),
        }),
        ..Default::default()
    };

    // Ensure we have output metadata slots for the new outputs.
    // new_psbt already has 3 output slots (from from_unsigned_tx), but they are default.
    // Output 0 was already restored above; outputs 1 and 2 remain default — that's fine.
    let _ = PsbtOutput::default(); // just to show the type is available

    Ok(BuyPsbt {
        psbt_hex: encode_psbt(&new_psbt),
        estimated_fee_sats,
    })
}

// ---------------------------------------------------------------------------
// Finalize and extract (bitmap-marketplace-jaa)
// ---------------------------------------------------------------------------

/// Take a fully-signed PSBT (buyer has added their signature), finalize it,
/// and return the raw transaction hex ready for broadcast.
///
/// Finalization moves `partial_sigs` into `final_script_sig` / `final_script_witness`
/// for each input, then extracts the network transaction.
pub fn finalize_and_extract(signed_psbt_hex: &str) -> Result<String> {
    let mut psbt = decode_psbt(signed_psbt_hex)?;

    // Manually finalize each input by promoting partial_sigs.
    for input in psbt.inputs.iter_mut() {
        // If already finalized, leave it alone.
        if input.final_script_sig.is_some() || input.final_script_witness.is_some() {
            continue;
        }

        // For segwit inputs: move partial signatures into final_script_witness.
        if !input.partial_sigs.is_empty() {
            let mut witness_items: Vec<Vec<u8>> = Vec::new();

            // BIP-174 / BIP-141: for P2WPKH the witness is [sig, pubkey].
            for (pubkey, sig) in &input.partial_sigs {
                witness_items.push(sig.to_vec());
                witness_items.push(pubkey.to_bytes());
            }

            input.final_script_witness = Some(Witness::from_slice(&witness_items));
            input.partial_sigs.clear();
        }
    }

    // Extract the finalized transaction.
    let final_tx = psbt
        .extract_tx()
        .map_err(|e| anyhow!("failed to extract transaction from PSBT: {}", e))?;

    // Serialize to raw transaction hex.
    let raw_bytes = bitcoin::consensus::encode::serialize(&final_tx);
    Ok(hex::encode(raw_bytes))
}

// ---------------------------------------------------------------------------
// Helpers (keep existing API)
// ---------------------------------------------------------------------------

pub fn decode_psbt(hex: &str) -> Result<Psbt> {
    let bytes = hex::decode(hex).map_err(|e| anyhow::anyhow!("invalid hex: {}", e))?;
    let psbt = Psbt::deserialize(&bytes)?;
    Ok(psbt)
}

pub fn encode_psbt(psbt: &Psbt) -> String {
    hex::encode(psbt.serialize())
}
