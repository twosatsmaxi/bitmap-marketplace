// PSBT construction for trustless listing and buying.
// bitmap-marketplace-ssd: listing construction
// bitmap-marketplace-jaa: buy flow completion

use anyhow::{anyhow, Result};
use bitcoin::{
    absolute::LockTime,
    opcodes::all as op,
    psbt::{Input as PsbtInput, Output as PsbtOutput, Psbt},
    script::Builder as ScriptBuilder,
    secp256k1::PublicKey,
    Address, Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    Witness,
};
use std::str::FromStr;

use crate::services::marketplace_keypair::MarketplaceKeypair;

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

// Mempool protection structs

pub struct LockingPsbtRequest {
    pub inscription_txid: String,
    pub inscription_vout: u32,
    pub inscription_amount_sats: u64,
    pub seller_pubkey_hex: String,
    pub marketplace_pubkey_hex: String,
    pub network: Network,
    /// Fee rate in sat/vB; if None, uses DEFAULT_LOCKING_TX_FEE_SATS floor.
    pub fee_rate_sat_vb: Option<f64>,
    /// Optional gas funding UTXO (required when inscription_amount_sats < MIN_SELF_FUNDED).
    pub gas_txid: Option<String>,
    pub gas_vout: Option<u32>,
    pub gas_amount_sats: Option<u64>,
}

pub struct LockingPsbt {
    pub psbt_hex: String,
    pub multisig_address: String,
    pub multisig_script_hex: String,
}

pub struct ProtectedSalePsbtRequest {
    /// Raw hex of the (not-yet-broadcast) locking transaction.
    pub locking_raw_tx_hex: String,
    /// Vout index of the multisig output in the locking tx (typically 0).
    pub multisig_vout: u32,
    pub multisig_script_hex: String,
    pub seller_address: String,
    pub price_sats: u64,
    pub buyer_address: String,
    pub buyer_utxo_txid: String,
    pub buyer_utxo_vout: u32,
    pub buyer_utxo_amount_sats: u64,
    pub fee_rate_sat_vb: f64,
}

pub struct ProtectedSalePsbt {
    pub psbt_hex: String,
    pub estimated_fee_sats: u64,
    /// Txid of the locking tx (derived from locking_raw_tx_hex).
    pub locking_txid: String,
}

/// Minimum inscription value (sats) that is self-funding for locking tx.
/// Below this, a gas funding UTXO must be provided.
pub const MIN_SELF_FUNDED: u64 = 343;
/// Fallback flat fee floor for locking tx if no dynamic estimate is available.
pub const DEFAULT_LOCKING_TX_FEE_SATS: u64 = 13;

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
// Mempool protection: locking tx PSBT
// ---------------------------------------------------------------------------

/// Sort two compressed pubkeys lexicographically (BIP-67) for deterministic multisig.
fn bip67_sort(a: &[u8; 33], b: &[u8; 33]) -> ([u8; 33], [u8; 33]) {
    if a <= b { (*a, *b) } else { (*b, *a) }
}

/// Build the P2WSH 2-of-2 multisig redeem script (witness script).
/// Keys are BIP-67 sorted for deterministic address derivation.
pub fn build_multisig_redeem_script(seller_pk: &PublicKey, marketplace_pk: &PublicKey) -> ScriptBuf {
    let (pk1, pk2) = bip67_sort(&seller_pk.serialize(), &marketplace_pk.serialize());
    ScriptBuilder::new()
        .push_opcode(op::OP_PUSHNUM_2)
        .push_slice(&pk1)
        .push_slice(&pk2)
        .push_opcode(op::OP_PUSHNUM_2)
        .push_opcode(op::OP_CHECKMULTISIG)
        .into_script()
}

/// Derive the P2WSH address from a redeem (witness) script.
pub fn p2wsh_address(witness_script: &ScriptBuf, network: Network) -> Address {
    Address::p2wsh(witness_script, network)
}

/// Build an unsigned locking PSBT: inscription UTXO → P2WSH 2-of-2 multisig.
/// The seller signs this with their normal sighash (SIGHASH_ALL).
/// The signed raw tx is stored in the DB; it is NOT broadcast until purchase time.
pub fn build_locking_psbt(req: &LockingPsbtRequest) -> Result<LockingPsbt> {
    let seller_pk = PublicKey::from_str(&req.seller_pubkey_hex)
        .map_err(|e| anyhow!("invalid seller_pubkey_hex: {}", e))?;
    let marketplace_pk = PublicKey::from_str(&req.marketplace_pubkey_hex)
        .map_err(|e| anyhow!("invalid marketplace_pubkey_hex: {}", e))?;

    let witness_script = build_multisig_redeem_script(&seller_pk, &marketplace_pk);
    let multisig_address = p2wsh_address(&witness_script, req.network);

    // Calculate locking tx fee.
    // Locking tx: 1 or 2 inputs, 1 output → estimate ~110–155 vbytes.
    let has_gas = req.gas_txid.is_some();
    let n_inputs = if has_gas { 2 } else { 1 };
    let fee_sats = if let Some(rate) = req.fee_rate_sat_vb {
        let vbytes = estimate_tx_vbytes(n_inputs, 1);
        std::cmp::max((vbytes as f64 * rate) as u64, DEFAULT_LOCKING_TX_FEE_SATS)
    } else {
        DEFAULT_LOCKING_TX_FEE_SATS
    };

    // The multisig output value equals inscription amount (self-funded) or gas amount minus fee.
    let inscription_txid = Txid::from_str(&req.inscription_txid)
        .map_err(|e| anyhow!("invalid inscription_txid: {}", e))?;
    let inscription_outpoint = OutPoint::new(inscription_txid, req.inscription_vout);

    let inscription_input = TxIn {
        previous_output: inscription_outpoint,
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::default(),
    };

    let multisig_value_sats = if has_gas {
        let gas_amt = req.gas_amount_sats.unwrap_or(0);
        req.inscription_amount_sats + gas_amt - fee_sats
    } else {
        if req.inscription_amount_sats < MIN_SELF_FUNDED {
            return Err(anyhow!(
                "inscription value ({} sats) is below MIN_SELF_FUNDED ({} sats); provide a gas UTXO",
                req.inscription_amount_sats,
                MIN_SELF_FUNDED
            ));
        }
        req.inscription_amount_sats - fee_sats
    };

    let multisig_output = TxOut {
        value: Amount::from_sat(multisig_value_sats),
        script_pubkey: multisig_address.script_pubkey(),
    };

    let mut inputs = vec![inscription_input];

    if has_gas {
        let gas_txid = Txid::from_str(req.gas_txid.as_deref().unwrap())
            .map_err(|e| anyhow!("invalid gas_txid: {}", e))?;
        let gas_input = TxIn {
            previous_output: OutPoint::new(gas_txid, req.gas_vout.unwrap_or(0)),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        };
        inputs.push(gas_input);
    }

    let unsigned_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: inputs,
        output: vec![multisig_output],
    };

    let mut psbt = Psbt::from_unsigned_tx(unsigned_tx)
        .map_err(|e| anyhow!("locking PSBT creation failed: {}", e))?;

    // Annotate input 0 with witness_utxo so seller's wallet can compute sighash.
    // We set the script_pubkey to the multisig script so it knows it's a P2WSH spend.
    // (The buyer-facing wallets will fill in UTXOs; this is just the template.)
    // Also set witness_script on all inputs so PSBT-aware wallets know what to sign.
    for input in psbt.inputs.iter_mut() {
        input.witness_script = Some(witness_script.clone());
    }

    Ok(LockingPsbt {
        psbt_hex: encode_psbt(&psbt),
        multisig_address: multisig_address.to_string(),
        multisig_script_hex: hex::encode(witness_script.as_bytes()),
    })
}

// ---------------------------------------------------------------------------
// Mempool protection: protected sale PSBT
// ---------------------------------------------------------------------------

/// Build the protected sale PSBT that spends from the (not-yet-broadcast) locking tx.
/// - Input 0: multisig output of locking tx (seller signs with SIGHASH_SINGLE | ANYONECANPAY)
/// - Input 1: buyer's payment UTXO (buyer signs normally)
/// - Output 0: payment to seller
/// - Output 1: inscription dust to buyer (546 sats)
/// - Output 2: change to buyer
pub fn build_protected_sale_psbt(req: &ProtectedSalePsbtRequest) -> Result<ProtectedSalePsbt> {
    // Decode the locking raw tx to get its txid and output value.
    let locking_tx_bytes = hex::decode(&req.locking_raw_tx_hex)
        .map_err(|e| anyhow!("invalid locking_raw_tx_hex: {}", e))?;
    let locking_tx: Transaction = bitcoin::consensus::deserialize(&locking_tx_bytes)
        .map_err(|e| anyhow!("cannot deserialize locking tx: {}", e))?;
    let locking_txid = locking_tx.txid();

    let multisig_txout = locking_tx
        .output
        .get(req.multisig_vout as usize)
        .ok_or_else(|| anyhow!("locking tx has no output at vout {}", req.multisig_vout))?;

    // Parse addresses.
    let seller_addr = Address::from_str(&req.seller_address)
        .map_err(|e| anyhow!("invalid seller_address: {}", e))?
        .assume_checked();
    let buyer_addr = Address::from_str(&req.buyer_address)
        .map_err(|e| anyhow!("invalid buyer_address: {}", e))?
        .assume_checked();

    // Parse witness script.
    let multisig_script_bytes = hex::decode(&req.multisig_script_hex)
        .map_err(|e| anyhow!("invalid multisig_script_hex: {}", e))?;
    let witness_script = ScriptBuf::from_bytes(multisig_script_bytes);

    const INSCRIPTION_DUST_SATS: u64 = 546;
    let estimated_fee_sats =
        (estimate_tx_vbytes(2, 3) as f64 * req.fee_rate_sat_vb) as u64;
    let total_required = req.price_sats + INSCRIPTION_DUST_SATS + estimated_fee_sats;

    if req.buyer_utxo_amount_sats < total_required {
        return Err(anyhow!(
            "buyer UTXO ({} sats) insufficient; need {} sats (price {} + dust {} + fee {})",
            req.buyer_utxo_amount_sats, total_required,
            req.price_sats, INSCRIPTION_DUST_SATS, estimated_fee_sats
        ));
    }

    let change_sats =
        req.buyer_utxo_amount_sats - req.price_sats - INSCRIPTION_DUST_SATS - estimated_fee_sats;

    // Build inputs.
    let multisig_input = TxIn {
        previous_output: OutPoint::new(locking_txid, req.multisig_vout),
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::default(),
    };

    let buyer_txid = Txid::from_str(&req.buyer_utxo_txid)
        .map_err(|e| anyhow!("invalid buyer_utxo_txid: {}", e))?;
    let buyer_input = TxIn {
        previous_output: OutPoint::new(buyer_txid, req.buyer_utxo_vout),
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::default(),
    };

    // Build outputs.
    let seller_output = TxOut {
        value: Amount::from_sat(req.price_sats),
        script_pubkey: seller_addr.script_pubkey(),
    };
    let inscription_output = TxOut {
        value: Amount::from_sat(INSCRIPTION_DUST_SATS),
        script_pubkey: buyer_addr.script_pubkey(),
    };
    let change_output = TxOut {
        value: Amount::from_sat(change_sats),
        script_pubkey: buyer_addr.script_pubkey(),
    };

    let unsigned_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![multisig_input, buyer_input],
        output: vec![seller_output, inscription_output, change_output],
    };

    let mut psbt = Psbt::from_unsigned_tx(unsigned_tx)
        .map_err(|e| anyhow!("protected sale PSBT creation failed: {}", e))?;

    // Annotate input 0 (multisig): SIGHASH_SINGLE | ANYONECANPAY, witness_utxo, witness_script.
    psbt.inputs[0].sighash_type = Some(bitcoin::psbt::PsbtSighashType::from(
        bitcoin::EcdsaSighashType::SinglePlusAnyoneCanPay,
    ));
    psbt.inputs[0].witness_utxo = Some(multisig_txout.clone());
    psbt.inputs[0].witness_script = Some(witness_script);

    // Annotate input 1 (buyer): witness_utxo.
    psbt.inputs[1] = PsbtInput {
        witness_utxo: Some(TxOut {
            value: Amount::from_sat(req.buyer_utxo_amount_sats),
            script_pubkey: ScriptBuf::new(), // buyer wallet fills in
        }),
        ..Default::default()
    };

    Ok(ProtectedSalePsbt {
        psbt_hex: encode_psbt(&psbt),
        estimated_fee_sats,
        locking_txid: locking_txid.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Mempool protection: marketplace co-signature + finalization
// ---------------------------------------------------------------------------

/// Apply the marketplace's SIGHASH_ALL co-signature on input 0 of the protected sale PSBT.
/// This completes the 2-of-2 multisig requirement on the seller's side.
pub fn apply_marketplace_signature(
    psbt_hex: &str,
    marketplace_keypair: &MarketplaceKeypair,
) -> Result<String> {
    use bitcoin::sighash::{EcdsaSighashType, SighashCache};

    let mut psbt = decode_psbt(psbt_hex)?;

    let witness_script = psbt.inputs[0]
        .witness_script
        .clone()
        .ok_or_else(|| anyhow!("input 0 missing witness_script"))?;
    let witness_utxo = psbt.inputs[0]
        .witness_utxo
        .clone()
        .ok_or_else(|| anyhow!("input 0 missing witness_utxo"))?;

    // Compute SIGHASH_ALL for input 0.
    let mut sighash_cache = SighashCache::new(&psbt.unsigned_tx);
    let sighash = sighash_cache
        .p2wsh_signature_hash(0, &witness_script, witness_utxo.value, EcdsaSighashType::All)
        .map_err(|e| anyhow!("sighash computation failed: {}", e))?;

    let sighash_slice: &[u8] = sighash.as_ref();
    let sighash_bytes: [u8; 32] = sighash_slice.try_into().expect("sighash is always 32 bytes");
    let sig = marketplace_keypair.sign_sighash(&sighash_bytes)?;
    let ecdsa_sig = bitcoin::ecdsa::Signature {
        sig,
        hash_ty: EcdsaSighashType::All,
    };

    let marketplace_pubkey = bitcoin::PublicKey {
        compressed: true,
        inner: marketplace_keypair.public_key(),
    };

    psbt.inputs[0]
        .partial_sigs
        .insert(marketplace_pubkey, ecdsa_sig);

    Ok(encode_psbt(&psbt))
}

/// Build the final P2WSH witness for input 0: [OP_0, seller_sig, marketplace_sig, redeem_script].
/// BIP-67 key sort determines sig order (sig for smaller pubkey goes first).
/// Returns the finalized raw transaction hex ready for broadcast.
pub fn finalize_multisig_and_extract(
    psbt_hex: &str,
    seller_pubkey_hex: &str,
    marketplace_pubkey_hex: &str,
) -> Result<String> {
    let mut psbt = decode_psbt(psbt_hex)?;

    let witness_script = psbt.inputs[0]
        .witness_script
        .clone()
        .ok_or_else(|| anyhow!("input 0 missing witness_script"))?;

    // Find partial sigs for seller and marketplace pubkeys.
    let seller_pk = bitcoin::PublicKey::from_str(seller_pubkey_hex)
        .map_err(|e| anyhow!("invalid seller_pubkey_hex: {}", e))?;
    let marketplace_pk = bitcoin::PublicKey::from_str(marketplace_pubkey_hex)
        .map_err(|e| anyhow!("invalid marketplace_pubkey_hex: {}", e))?;

    let seller_sig = psbt.inputs[0]
        .partial_sigs
        .get(&seller_pk)
        .ok_or_else(|| anyhow!("seller signature missing from PSBT input 0"))?
        .clone();
    let marketplace_sig = psbt.inputs[0]
        .partial_sigs
        .get(&marketplace_pk)
        .ok_or_else(|| anyhow!("marketplace signature missing from PSBT input 0"))?
        .clone();

    // BIP-67 sort determines which sig goes first in witness (matches key order in script).
    let (first_sig, second_sig) = {
        let seller_bytes = seller_pk.inner.serialize();
        let marketplace_bytes = marketplace_pk.inner.serialize();
        if seller_bytes <= marketplace_bytes {
            (seller_sig, marketplace_sig)
        } else {
            (marketplace_sig, seller_sig)
        }
    };

    // P2WSH OP_CHECKMULTISIG witness: [OP_0 (empty bytes), sig1, sig2, witness_script]
    let witness = Witness::from_slice(&[
        &[][..],              // OP_0 / dummy for CHECKMULTISIG bug
        &first_sig.to_vec(),
        &second_sig.to_vec(),
        witness_script.as_bytes(),
    ]);

    psbt.inputs[0].final_script_witness = Some(witness);
    psbt.inputs[0].partial_sigs.clear();
    psbt.inputs[0].witness_script = None; // moved into final witness

    // Finalize remaining inputs (buyer's input).
    for input in psbt.inputs.iter_mut().skip(1) {
        if input.final_script_sig.is_some() || input.final_script_witness.is_some() {
            continue;
        }
        if !input.partial_sigs.is_empty() {
            let mut items: Vec<Vec<u8>> = Vec::new();
            for (pubkey, sig) in &input.partial_sigs {
                items.push(sig.to_vec());
                items.push(pubkey.to_bytes());
            }
            input.final_script_witness = Some(Witness::from_slice(
                &items.iter().map(|v| v.as_slice()).collect::<Vec<_>>(),
            ));
            input.partial_sigs.clear();
        }
    }

    let final_tx = psbt
        .extract_tx()
        .map_err(|e| anyhow!("failed to extract finalized tx: {}", e))?;
    let raw_bytes = bitcoin::consensus::encode::serialize(&final_tx);
    Ok(hex::encode(raw_bytes))
}

// ---------------------------------------------------------------------------
// Locking PSBT finalization helper
// ---------------------------------------------------------------------------

/// Finalize a seller-signed locking PSBT and return the raw transaction hex.
/// The locking tx uses standard SIGHASH_ALL (P2WPKH or P2TR input from seller).
/// We promote partial_sigs to final_script_witness and extract the tx.
pub fn finalize_locking_psbt(signed_psbt_hex: &str) -> Result<String> {
    let mut psbt = decode_psbt(signed_psbt_hex)?;

    for input in psbt.inputs.iter_mut() {
        if input.final_script_sig.is_some() || input.final_script_witness.is_some() {
            continue;
        }
        if !input.partial_sigs.is_empty() {
            let mut items: Vec<Vec<u8>> = Vec::new();
            for (pubkey, sig) in &input.partial_sigs {
                items.push(sig.to_vec());
                items.push(pubkey.to_bytes());
            }
            input.final_script_witness = Some(Witness::from_slice(
                &items.iter().map(|v| v.as_slice()).collect::<Vec<_>>(),
            ));
            input.partial_sigs.clear();
        }
    }

    let final_tx = psbt
        .extract_tx()
        .map_err(|e| anyhow!("failed to extract locking tx: {}", e))?;
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

#[cfg(test)]
#[path = "psbt_tests.rs"]
mod psbt_tests;
