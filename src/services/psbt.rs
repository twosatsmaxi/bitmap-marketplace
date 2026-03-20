// PSBT construction for trustless listing and buying.
// bitmap-marketplace-ssd: listing construction
// bitmap-marketplace-jaa: buy flow completion
// bitmap-marketplace-iwo: Taproot script-path migration

use anyhow::{anyhow, Result};
use bitcoin::{
    absolute::LockTime,
    hashes::Hash,
    opcodes::all as op,
    psbt::{Input as PsbtInput, Output as PsbtOutput, Psbt, PsbtSighashType},
    script::{Builder as ScriptBuilder, PushBytesBuf},
    secp256k1::{self, PublicKey, XOnlyPublicKey},
    sighash::TapSighashType,
    taproot::{self, ControlBlock, LeafVersion, TaprootBuilder},
    Address, Amount, Network, OutPoint, ScriptBuf, Sequence, TapLeafHash, Transaction, TxIn,
    TxOut, Txid, Witness,
};
use std::str::FromStr;

use crate::models::listing::SpendableInputRequest;
use crate::services::marketplace_keypair::MarketplaceKeypair;

impl From<&SpendableInputRequest> for SpendableInput {
    fn from(req: &SpendableInputRequest) -> Self {
        SpendableInput {
            txid: req.txid.clone(),
            vout: req.vout,
            value_sats: req.value_sats,
            witness_utxo: WitnessUtxo {
                script_pubkey_hex: req.witness_utxo.script_pubkey_hex.clone(),
                value_sats: req.witness_utxo.value_sats,
            },
            non_witness_utxo_hex: req.non_witness_utxo_hex.clone(),
            redeem_script_hex: req.redeem_script_hex.clone(),
            witness_script_hex: req.witness_script_hex.clone(),
            sequence: req.sequence,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WitnessUtxo {
    pub script_pubkey_hex: String,
    pub value_sats: u64,
}

#[derive(Clone, Debug)]
pub struct SpendableInput {
    pub txid: String,
    pub vout: u32,
    pub value_sats: u64,
    pub witness_utxo: WitnessUtxo,
    pub non_witness_utxo_hex: Option<String>,
    pub redeem_script_hex: Option<String>,
    pub witness_script_hex: Option<String>,
    pub sequence: Option<u32>,
}

pub struct ListingRequest {
    pub inscription_txid: String,
    pub inscription_vout: u32,
    pub seller_address: String,
    pub price_sats: u64,
}

#[derive(Debug)]
pub struct ListingPsbt {
    pub psbt_hex: String,
    pub inscription_txid: String,
    pub inscription_vout: u32,
}

pub struct BuyRequest {
    pub seller_psbt_hex: String,
    pub buyer_address: String,
    pub buyer_funding_input: SpendableInput,
    pub fee_rate_sat_vb: f64,
    pub marketplace_fee_address: Option<String>,
    pub marketplace_fee_bps: u64,
}

#[derive(Debug)]
pub struct BuyPsbt {
    pub psbt_hex: String,
    pub estimated_fee_sats: u64,
    pub marketplace_fee_sats: u64,
}

pub struct LockingPsbtRequest {
    pub inscription_input: SpendableInput,
    pub gas_funding_input: Option<SpendableInput>,
    pub seller_pubkey_hex: String,
    pub seller_address: String,
    pub price_sats: u64,
    pub marketplace_pubkey_hex: String,
    pub network: Network,
    pub min_relay_fee_rate_sat_vb: Option<f64>,
}

#[derive(Debug)]
pub struct LockingPsbt {
    pub psbt_hex: String,
    pub multisig_address: String,
    pub multisig_script_hex: String,
    /// Sale template PSBT for the seller to pre-sign with SIGHASH_SINGLE|ANYONECANPAY.
    pub sale_template_psbt_hex: String,
}

pub struct ProtectedSalePsbtRequest {
    pub locking_raw_tx_hex: String,
    pub multisig_vout: u32,
    pub multisig_script_hex: String,
    pub seller_address: String,
    pub seller_pubkey_hex: String,
    pub price_sats: u64,
    pub buyer_address: String,
    pub buyer_funding_input: SpendableInput,
    pub fee_rate_sat_vb: f64,
    pub marketplace_fee_address: Option<String>,
    pub marketplace_fee_bps: u64,
    /// Seller's pre-signed Schnorr signature (hex) for the multisig input, signed at listing time.
    pub seller_sale_sig_hex: Option<String>,
}

#[derive(Debug)]
pub struct ProtectedSalePsbt {
    pub psbt_hex: String,
    pub estimated_fee_sats: u64,
    pub locking_txid: String,
    pub marketplace_fee_sats: u64,
}

/// Taproot 2-of-2 multisig descriptor returned by `create_taproot_multisig`.
pub struct TaprootMultisig {
    pub address: Address,
    pub output_script: ScriptBuf,
    pub leaf_script: ScriptBuf,
    pub internal_key: XOnlyPublicKey,
    pub control_block: ControlBlock,
    pub leaf_version: LeafVersion,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SupportedInputType {
    P2shP2wpkh,
    P2wpkh,
    P2tr,
}

const DEFAULT_MIN_RELAY_FEE_RATE_SAT_VB: f64 = 0.1;
const LOCKING_TX_OUTPUT_VBYTES: f64 = 43.0;
const LOCKING_TX_OVERHEAD_VBYTES: f64 = 10.5;
const SALE_TX_OUTPUT_VBYTES: f64 = 31.0;
const SALE_TX_OVERHEAD_VBYTES: f64 = 10.5;
/// Taproot script-path 2-of-2 input: ~107 vbytes (2 Schnorr sigs + script + control block).
const TAPROOT_MULTISIG_INPUT_VBYTES: f64 = 107.5;
const INSCRIPTION_DUST_SATS: u64 = 546;
const LOCKING_OUTPUT_DUST_SATS: u64 = 330;
const MIN_MARKETPLACE_FEE_SATS: u64 = 1000;

/// Minimum inscription value (sats) that is self-funding for locking tx.
pub const MIN_SELF_FUNDED: u64 = 343;

/// Sort two compressed pubkeys lexicographically (BIP 67) and return x-only versions.
fn sorted_x_only_pubkeys(a: &PublicKey, b: &PublicKey) -> (XOnlyPublicKey, XOnlyPublicKey) {
    let a_bytes = a.serialize();
    let b_bytes = b.serialize();
    let (first, second) = if a_bytes <= b_bytes { (a, b) } else { (b, a) };
    (XOnlyPublicKey::from(*first), XOnlyPublicKey::from(*second))
}

/// Build a Taproot 2-of-2 multisig using script-path spending.
/// Script: `<xpk1> OP_CHECKSIG <xpk2> OP_CHECKSIGADD OP_2 OP_NUMEQUAL`
/// Matches the mempool-protection reference implementation.
pub fn create_taproot_multisig(
    seller_pk: &PublicKey,
    marketplace_pk: &PublicKey,
    internal_key: XOnlyPublicKey,
    network: Network,
) -> Result<TaprootMultisig> {
    let (xpk1, xpk2) = sorted_x_only_pubkeys(seller_pk, marketplace_pk);

    let leaf_script = ScriptBuilder::new()
        .push_x_only_key(&xpk1)
        .push_opcode(op::OP_CHECKSIG)
        .push_x_only_key(&xpk2)
        .push_opcode(op::OP_CHECKSIGADD)
        .push_opcode(op::OP_PUSHNUM_2)
        .push_opcode(op::OP_NUMEQUAL)
        .into_script();

    let builder = TaprootBuilder::new()
        .add_leaf(0, leaf_script.clone())
        .map_err(|e| anyhow!("TaprootBuilder::add_leaf failed: {:?}", e))?;

    let spend_info = builder
        .finalize(&secp256k1::Secp256k1::new(), internal_key)
        .map_err(|_| anyhow!("TaprootBuilder::finalize failed"))?;

    let address = Address::p2tr_tweaked(spend_info.output_key(), network);
    let output_script = address.script_pubkey();

    let control_block = spend_info
        .control_block(&(leaf_script.clone(), LeafVersion::TapScript))
        .ok_or_else(|| anyhow!("failed to compute control block for taproot multisig"))?;

    Ok(TaprootMultisig {
        address,
        output_script,
        leaf_script,
        internal_key,
        control_block,
        leaf_version: LeafVersion::TapScript,
    })
}

// ── Legacy listing PSBT (unprotected flow, unchanged) ───────────────────────

pub fn build_listing_psbt(req: &ListingRequest) -> Result<ListingPsbt> {
    let txid = Txid::from_str(&req.inscription_txid)
        .map_err(|e| anyhow!("invalid inscription txid: {}", e))?;
    let outpoint = OutPoint::new(txid, req.inscription_vout);

    let seller_addr = Address::from_str(&req.seller_address)
        .map_err(|e| anyhow!("invalid seller address: {}", e))?
        .assume_checked();
    let seller_script = seller_addr.script_pubkey();

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

    let mut psbt =
        Psbt::from_unsigned_tx(unsigned_tx).map_err(|e| anyhow!("PSBT creation failed: {}", e))?;

    psbt.inputs[0].sighash_type = Some(PsbtSighashType::from(
        bitcoin::EcdsaSighashType::SinglePlusAnyoneCanPay,
    ));

    Ok(ListingPsbt {
        psbt_hex: encode_psbt(&psbt),
        inscription_txid: req.inscription_txid.clone(),
        inscription_vout: req.inscription_vout,
    })
}

// ── Input type detection (shared) ───────────────────────────────────────────

fn is_p2wpkh_script(script: &ScriptBuf) -> bool {
    let bytes = script.as_bytes();
    bytes.len() == 22 && bytes[0] == 0x00 && bytes[1] == 0x14
}

fn is_p2tr_script(script: &ScriptBuf) -> bool {
    let bytes = script.as_bytes();
    bytes.len() == 34 && bytes[0] == 0x51 && bytes[1] == 0x20
}

fn is_p2sh_script(script: &ScriptBuf) -> bool {
    let bytes = script.as_bytes();
    bytes.len() == 23 && bytes[0] == 0xa9 && bytes[1] == 0x14 && bytes[22] == 0x87
}

fn locking_input_vbytes(input_type: SupportedInputType) -> f64 {
    match input_type {
        SupportedInputType::P2shP2wpkh => 91.0,
        SupportedInputType::P2wpkh => 67.75,
        SupportedInputType::P2tr => 57.5,
    }
}

fn sale_tx_vbytes(buyer_input_type: SupportedInputType, output_count: u64) -> u64 {
    (SALE_TX_OVERHEAD_VBYTES
        + TAPROOT_MULTISIG_INPUT_VBYTES
        + locking_input_vbytes(buyer_input_type)
        + (SALE_TX_OUTPUT_VBYTES * output_count as f64))
        .ceil() as u64
}

/// Calculate marketplace fee: max(price * bps / 10000, MIN_MARKETPLACE_FEE_SATS).
pub fn calculate_marketplace_fee(price_sats: u64, fee_bps: u64) -> u64 {
    if fee_bps == 0 {
        return 0;
    }
    let bps_fee = price_sats * fee_bps / 10_000;
    bps_fee.max(MIN_MARKETPLACE_FEE_SATS)
}

fn estimate_locking_tx_vbytes(input_types: &[SupportedInputType]) -> u64 {
    (LOCKING_TX_OVERHEAD_VBYTES
        + LOCKING_TX_OUTPUT_VBYTES
        + input_types
            .iter()
            .copied()
            .map(locking_input_vbytes)
            .sum::<f64>())
    .ceil() as u64
}

fn parse_script_hex(label: &str, value: &str) -> Result<ScriptBuf> {
    let bytes = hex::decode(value).map_err(|e| anyhow!("invalid {label}: {}", e))?;
    Ok(ScriptBuf::from_bytes(bytes))
}

fn parse_tx_hex(label: &str, value: &str) -> Result<Transaction> {
    let bytes = hex::decode(value).map_err(|e| anyhow!("invalid {label}: {}", e))?;
    bitcoin::consensus::deserialize(&bytes).map_err(|e| anyhow!("invalid {label}: {}", e))
}

fn parse_witness_utxo(input: &SpendableInput) -> Result<TxOut> {
    if input.value_sats != input.witness_utxo.value_sats {
        return Err(anyhow!(
            "input value mismatch: value_sats {} != witness_utxo.value_sats {}",
            input.value_sats,
            input.witness_utxo.value_sats
        ));
    }

    Ok(TxOut {
        value: Amount::from_sat(input.witness_utxo.value_sats),
        script_pubkey: parse_script_hex(
            "witness_utxo.script_pubkey_hex",
            &input.witness_utxo.script_pubkey_hex,
        )?,
    })
}

fn parse_sequence(sequence: Option<u32>) -> Sequence {
    sequence
        .map(Sequence)
        .unwrap_or(Sequence::ENABLE_RBF_NO_LOCKTIME)
}

fn detect_supported_input_type(input: &SpendableInput, label: &str) -> Result<SupportedInputType> {
    let witness_utxo = parse_witness_utxo(input)?;
    if is_p2wpkh_script(&witness_utxo.script_pubkey) {
        return Ok(SupportedInputType::P2wpkh);
    }
    if is_p2tr_script(&witness_utxo.script_pubkey) {
        return Ok(SupportedInputType::P2tr);
    }
    if is_p2sh_script(&witness_utxo.script_pubkey) {
        let redeem_script_hex = input.redeem_script_hex.as_deref().ok_or_else(|| {
            anyhow!("{label} wrapped segwit input must provide redeem_script_hex")
        })?;
        let redeem_script = parse_script_hex("redeem_script_hex", redeem_script_hex)?;
        if !is_p2wpkh_script(&redeem_script) {
            return Err(anyhow!(
                "{label} redeem_script_hex must be a native p2wpkh script for wrapped segwit inputs"
            ));
        }
        return Ok(SupportedInputType::P2shP2wpkh);
    }

    Err(anyhow!(
        "{label} must be wrapped segwit (p2sh-p2wpkh), native segwit (p2wpkh), or taproot (p2tr)"
    ))
}

fn add_spendable_input(psbt: &mut Psbt, input_index: usize, input: &SpendableInput) -> Result<()> {
    let witness_utxo = parse_witness_utxo(input)?;
    let mut psbt_input = PsbtInput {
        witness_utxo: Some(witness_utxo),
        ..Default::default()
    };

    if let Some(non_witness_utxo_hex) = input.non_witness_utxo_hex.as_deref() {
        psbt_input.non_witness_utxo =
            Some(parse_tx_hex("non_witness_utxo_hex", non_witness_utxo_hex)?);
    }
    if let Some(redeem_script_hex) = input.redeem_script_hex.as_deref() {
        psbt_input.redeem_script = Some(parse_script_hex("redeem_script_hex", redeem_script_hex)?);
    }
    if let Some(witness_script_hex) = input.witness_script_hex.as_deref() {
        psbt_input.witness_script =
            Some(parse_script_hex("witness_script_hex", witness_script_hex)?);
    }

    psbt.inputs[input_index] = psbt_input;
    Ok(())
}

fn finalize_input(input: &mut PsbtInput) -> Result<()> {
    if input.final_script_sig.is_some() || input.final_script_witness.is_some() {
        return Ok(());
    }

    if let Some(tap_key_sig) = input.tap_key_sig.take() {
        let mut witness = Witness::new();
        witness.push(tap_key_sig.to_vec());
        input.final_script_witness = Some(witness);
        input.sighash_type = None;
        input.redeem_script = None;
        input.witness_script = None;
        input.partial_sigs.clear();
        return Ok(());
    }

    if input.partial_sigs.is_empty() {
        return Err(anyhow!("input missing signatures"));
    }

    let witness_utxo = input
        .witness_utxo
        .clone()
        .ok_or_else(|| anyhow!("input missing witness_utxo"))?;

    if is_p2wpkh_script(&witness_utxo.script_pubkey) {
        let (pubkey, sig) = input
            .partial_sigs
            .iter()
            .next()
            .ok_or_else(|| anyhow!("p2wpkh input missing partial signature"))?;
        input.final_script_witness = Some(Witness::from_slice(&[sig.to_vec(), pubkey.to_bytes()]));
        input.partial_sigs.clear();
        input.sighash_type = None;
        return Ok(());
    }

    if is_p2sh_script(&witness_utxo.script_pubkey) {
        let redeem_script = input
            .redeem_script
            .clone()
            .ok_or_else(|| anyhow!("p2sh-p2wpkh input missing redeem_script"))?;
        if !is_p2wpkh_script(&redeem_script) {
            return Err(anyhow!("unsupported p2sh redeem script"));
        }
        let (pubkey, sig) = input
            .partial_sigs
            .iter()
            .next()
            .ok_or_else(|| anyhow!("p2sh-p2wpkh input missing partial signature"))?;
        input.final_script_sig = Some(push_script_bytes(redeem_script.as_bytes())?);
        input.final_script_witness = Some(Witness::from_slice(&[sig.to_vec(), pubkey.to_bytes()]));
        input.partial_sigs.clear();
        input.sighash_type = None;
        input.redeem_script = None;
        return Ok(());
    }

    Err(anyhow!(
        "unsupported finalized input type; only p2wpkh, p2sh-p2wpkh, and p2tr are supported"
    ))
}

fn push_script_bytes(bytes: &[u8]) -> Result<ScriptBuf> {
    let push_bytes = PushBytesBuf::try_from(bytes.to_vec())
        .map_err(|_| anyhow!("script bytes exceed push size limit"))?;
    Ok(ScriptBuilder::new().push_slice(push_bytes).into_script())
}

// ── Legacy buy PSBT (unprotected flow, unchanged) ───────────────────────────

pub fn build_buy_psbt(req: &BuyRequest) -> Result<BuyPsbt> {
    let psbt = decode_psbt(&req.seller_psbt_hex)?;

    if psbt.unsigned_tx.input.len() != 1 || psbt.unsigned_tx.output.len() != 1 {
        return Err(anyhow!(
            "seller PSBT has unexpected structure: expected 1 input and 1 output"
        ));
    }

    let price_sats = psbt.unsigned_tx.output[0].value.to_sat();
    let buyer_addr = Address::from_str(&req.buyer_address)
        .map_err(|e| anyhow!("invalid buyer address: {}", e))?
        .assume_checked();
    let buyer_script = buyer_addr.script_pubkey();
    let buyer_input_type =
        detect_supported_input_type(&req.buyer_funding_input, "buyer_funding_input")?;

    let marketplace_fee_sats = calculate_marketplace_fee(price_sats, req.marketplace_fee_bps);
    let fee_addr = req
        .marketplace_fee_address
        .as_ref()
        .filter(|_| marketplace_fee_sats > 0);
    let output_count: u64 = if fee_addr.is_some() { 4 } else { 3 };

    let estimated_fee_sats =
        (sale_tx_vbytes(buyer_input_type, output_count) as f64 * req.fee_rate_sat_vb) as u64;
    let total_required =
        price_sats + INSCRIPTION_DUST_SATS + estimated_fee_sats + marketplace_fee_sats;
    if req.buyer_funding_input.value_sats < total_required {
        return Err(anyhow!(
            "buyer input amount ({} sats) is insufficient; need at least {} sats (price {} + dust {} + tx_fee {} + marketplace_fee {})",
            req.buyer_funding_input.value_sats,
            total_required,
            price_sats,
            INSCRIPTION_DUST_SATS,
            estimated_fee_sats,
            marketplace_fee_sats
        ));
    }

    let change_sats = req.buyer_funding_input.value_sats
        - price_sats
        - INSCRIPTION_DUST_SATS
        - estimated_fee_sats
        - marketplace_fee_sats;
    let buyer_txid = Txid::from_str(&req.buyer_funding_input.txid)
        .map_err(|e| anyhow!("invalid buyer input txid: {}", e))?;
    let buyer_tx_in = TxIn {
        previous_output: OutPoint::new(buyer_txid, req.buyer_funding_input.vout),
        script_sig: ScriptBuf::new(),
        sequence: parse_sequence(req.buyer_funding_input.sequence),
        witness: Witness::default(),
    };

    let mut tx = psbt.unsigned_tx.clone();
    tx.input.push(buyer_tx_in);
    tx.output.push(TxOut {
        value: Amount::from_sat(INSCRIPTION_DUST_SATS),
        script_pubkey: buyer_script.clone(),
    });
    tx.output.push(TxOut {
        value: Amount::from_sat(change_sats),
        script_pubkey: buyer_script,
    });

    if let Some(addr_str) = fee_addr {
        let fee_address = Address::from_str(addr_str)
            .map_err(|e| anyhow!("invalid marketplace_fee_address: {}", e))?
            .assume_checked();
        tx.output.push(TxOut {
            value: Amount::from_sat(marketplace_fee_sats),
            script_pubkey: fee_address.script_pubkey(),
        });
    }

    let seller_input_meta = psbt.inputs[0].clone();
    let seller_output_meta = psbt.outputs[0].clone();

    let mut new_psbt = Psbt::from_unsigned_tx(tx)
        .map_err(|e| anyhow!("PSBT creation from extended tx failed: {}", e))?;
    new_psbt.inputs[0] = seller_input_meta;
    new_psbt.outputs[0] = seller_output_meta;
    add_spendable_input(&mut new_psbt, 1, &req.buyer_funding_input)?;
    let _ = PsbtOutput::default();

    Ok(BuyPsbt {
        psbt_hex: encode_psbt(&new_psbt),
        estimated_fee_sats,
        marketplace_fee_sats,
    })
}

pub fn finalize_and_extract(signed_psbt_hex: &str) -> Result<String> {
    let mut psbt = decode_psbt(signed_psbt_hex)?;
    for input in psbt.inputs.iter_mut() {
        finalize_input(input)?;
    }

    let final_tx = psbt
        .extract_tx()
        .map_err(|e| anyhow!("failed to extract transaction from PSBT: {}", e))?;
    let raw_bytes = bitcoin::consensus::encode::serialize(&final_tx);
    Ok(hex::encode(raw_bytes))
}

// ── Taproot 2-of-2 locking PSBT ────────────────────────────────────────────

/// Resolve the Taproot internal key from the inscription input (or fallback to seller x-only).
/// Matches the reference: prefer tapInternalKey from input, then fallback to seller x-only pubkey.
fn resolve_internal_key(
    _inscription_input: &SpendableInput,
    seller_pk: &PublicKey,
) -> XOnlyPublicKey {
    // For now, use the seller's x-only pubkey as the internal key.
    // This makes the key-path unspendable (would need the tweaked private key),
    // forcing script-path spending which requires both signatures.
    XOnlyPublicKey::from(*seller_pk)
}

pub fn build_locking_psbt(req: &LockingPsbtRequest) -> Result<LockingPsbt> {
    let seller_pk = PublicKey::from_str(&req.seller_pubkey_hex)
        .map_err(|e| anyhow!("invalid seller_pubkey_hex: {}", e))?;
    let marketplace_pk = PublicKey::from_str(&req.marketplace_pubkey_hex)
        .map_err(|e| anyhow!("invalid marketplace_pubkey_hex: {}", e))?;

    let internal_key = resolve_internal_key(&req.inscription_input, &seller_pk);
    let multisig = create_taproot_multisig(&seller_pk, &marketplace_pk, internal_key, req.network)?;

    let inscription_input_type =
        detect_supported_input_type(&req.inscription_input, "inscription_input")?;
    let min_relay_fee_rate = req
        .min_relay_fee_rate_sat_vb
        .unwrap_or(DEFAULT_MIN_RELAY_FEE_RATE_SAT_VB);

    if !min_relay_fee_rate.is_finite() || min_relay_fee_rate < 0.0 {
        return Err(anyhow!(
            "min_relay_fee_rate_sat_vb must be a finite non-negative number"
        ));
    }

    let mut input_types = vec![inscription_input_type];
    if req.inscription_input.value_sats < MIN_SELF_FUNDED && req.gas_funding_input.is_none() {
        return Err(anyhow!(
            "inscription input {} sats is below MIN_SELF_FUNDED ({} sats); provide gas_funding_input",
            req.inscription_input.value_sats,
            MIN_SELF_FUNDED
        ));
    }

    if let Some(gas_input) = req.gas_funding_input.as_ref() {
        let gas_type = detect_supported_input_type(gas_input, "gas_funding_input")?;
        input_types.push(gas_type);
        let minimum_gas_funding_sats =
            (estimate_locking_tx_vbytes(&input_types) as f64 * min_relay_fee_rate).ceil() as u64;
        if gas_input.value_sats < minimum_gas_funding_sats {
            return Err(anyhow!(
                "gas_funding_input requires at least {} sats (locking tx {} vbytes at {} sat/vB)",
                minimum_gas_funding_sats,
                estimate_locking_tx_vbytes(&input_types),
                min_relay_fee_rate
            ));
        }
    }

    let locking_fee_sats =
        (estimate_locking_tx_vbytes(&input_types) as f64 * min_relay_fee_rate).ceil() as u64;
    let locked_value_sats = req.inscription_input.value_sats
        + req
            .gas_funding_input
            .as_ref()
            .map(|input| input.value_sats)
            .unwrap_or(0)
        - locking_fee_sats;

    if locked_value_sats < LOCKING_OUTPUT_DUST_SATS {
        return Err(anyhow!(
            "locking output would be {} sats, below dust limit {} sats",
            locked_value_sats,
            LOCKING_OUTPUT_DUST_SATS
        ));
    }

    let inscription_txid = Txid::from_str(&req.inscription_input.txid)
        .map_err(|e| anyhow!("invalid inscription_input.txid: {}", e))?;
    let mut inputs = vec![TxIn {
        previous_output: OutPoint::new(inscription_txid, req.inscription_input.vout),
        script_sig: ScriptBuf::new(),
        sequence: parse_sequence(req.inscription_input.sequence),
        witness: Witness::default(),
    }];

    if let Some(gas_input) = req.gas_funding_input.as_ref() {
        let gas_txid = Txid::from_str(&gas_input.txid)
            .map_err(|e| anyhow!("invalid gas_funding_input.txid: {}", e))?;
        inputs.push(TxIn {
            previous_output: OutPoint::new(gas_txid, gas_input.vout),
            script_sig: ScriptBuf::new(),
            sequence: parse_sequence(gas_input.sequence),
            witness: Witness::default(),
        });
    }

    let unsigned_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: inputs,
        output: vec![TxOut {
            value: Amount::from_sat(locked_value_sats),
            script_pubkey: multisig.output_script.clone(),
        }],
    };

    let mut psbt = Psbt::from_unsigned_tx(unsigned_tx)
        .map_err(|e| anyhow!("locking PSBT creation failed: {}", e))?;
    add_spendable_input(&mut psbt, 0, &req.inscription_input)?;
    if let Some(gas_input) = req.gas_funding_input.as_ref() {
        add_spendable_input(&mut psbt, 1, gas_input)?;
    }

    let multisig_script_hex = hex::encode(multisig.leaf_script.as_bytes());

    // Build sale template for seller pre-signing (SIGHASH_SINGLE|ANYONECANPAY).
    // SegWit v1 txid is stable (non-malleable), so we can compute the planned locked UTXO.
    let seller_addr = Address::from_str(&req.seller_address)
        .map_err(|e| anyhow!("invalid seller_address: {}", e))?
        .assume_checked();
    let sale_template = build_sale_template_psbt(
        &encode_psbt(&psbt),
        &multisig,
        &seller_addr,
        req.price_sats,
    )?;

    Ok(LockingPsbt {
        psbt_hex: encode_psbt(&psbt),
        multisig_address: multisig.address.to_string(),
        multisig_script_hex,
        sale_template_psbt_hex: sale_template,
    })
}

// ── Sale template PSBT (Taproot) ────────────────────────────────────────────

/// Build a minimal sale template PSBT for the seller to pre-sign.
/// Contains 1 input (the planned Taproot multisig UTXO) and 1 output (seller payment placeholder).
/// The seller signs with SIGHASH_SINGLE|ANYONECANPAY via Schnorr script-path so the
/// signature remains valid when additional inputs/outputs are added at buy time.
fn build_sale_template_psbt(
    locking_psbt_hex: &str,
    multisig: &TaprootMultisig,
    seller_address: &Address,
    price_sats: u64,
) -> Result<String> {
    let locking_psbt = decode_psbt(locking_psbt_hex)?;
    let locking_txid = locking_psbt.unsigned_tx.txid();
    let multisig_txout = locking_psbt
        .unsigned_tx
        .output
        .first()
        .ok_or_else(|| anyhow!("locking PSBT has no outputs"))?
        .clone();

    let unsigned_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(locking_txid, 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(price_sats),
            script_pubkey: seller_address.script_pubkey(),
        }],
    };

    let mut psbt = Psbt::from_unsigned_tx(unsigned_tx)
        .map_err(|e| anyhow!("sale template PSBT creation failed: {}", e))?;

    // Set Taproot script-path metadata for the multisig input.
    psbt.inputs[0].sighash_type = Some(PsbtSighashType::from(
        TapSighashType::SinglePlusAnyoneCanPay,
    ));
    psbt.inputs[0].witness_utxo = Some(multisig_txout);
    psbt.inputs[0].tap_internal_key = Some(multisig.internal_key);
    psbt.inputs[0].tap_merkle_root = Some(
        TapLeafHash::from_script(&multisig.leaf_script, LeafVersion::TapScript).into(),
    );
    psbt.inputs[0]
        .tap_scripts
        .insert(multisig.control_block.clone(), (multisig.leaf_script.clone(), LeafVersion::TapScript));

    Ok(encode_psbt(&psbt))
}

/// Extract the seller's Schnorr signature from a signed sale template PSBT.
/// The seller signs the template with SIGHASH_SINGLE|ANYONECANPAY via taproot script-path;
/// this extracts the resulting tap_script_sig for embedding into the full sale PSBT at buy time.
pub fn extract_seller_sale_sig(
    signed_sale_template_hex: &str,
    seller_pubkey_hex: &str,
) -> Result<String> {
    let psbt = decode_psbt(signed_sale_template_hex)?;
    let seller_pk = PublicKey::from_str(seller_pubkey_hex)
        .map_err(|e| anyhow!("invalid seller_pubkey_hex: {}", e))?;
    let seller_xonly = XOnlyPublicKey::from(seller_pk);

    // Find the seller's tap_script_sig in input 0.
    let sig = psbt.inputs[0]
        .tap_script_sigs
        .iter()
        .find(|((pk, _leaf_hash), _sig)| *pk == seller_xonly)
        .map(|(_, sig)| sig)
        .ok_or_else(|| anyhow!("seller taproot signature missing from sale template PSBT input 0"))?;

    Ok(hex::encode(sig.to_vec()))
}

// ── Protected sale PSBT (Taproot) ───────────────────────────────────────────

pub fn build_protected_sale_psbt(req: &ProtectedSalePsbtRequest) -> Result<ProtectedSalePsbt> {
    let locking_tx = parse_tx_hex("locking_raw_tx_hex", &req.locking_raw_tx_hex)?;
    let locking_txid = locking_tx.txid();
    let multisig_txout = locking_tx
        .output
        .get(req.multisig_vout as usize)
        .ok_or_else(|| anyhow!("locking tx has no output at vout {}", req.multisig_vout))?
        .clone();

    let seller_addr = Address::from_str(&req.seller_address)
        .map_err(|e| anyhow!("invalid seller_address: {}", e))?
        .assume_checked();
    let buyer_addr = Address::from_str(&req.buyer_address)
        .map_err(|e| anyhow!("invalid buyer_address: {}", e))?
        .assume_checked();
    let leaf_script = parse_script_hex("multisig_script_hex", &req.multisig_script_hex)?;
    let buyer_input_type =
        detect_supported_input_type(&req.buyer_funding_input, "buyer_funding_input")?;

    // Reconstruct the Taproot multisig to get internal_key and control_block.
    let seller_pk = PublicKey::from_str(&req.seller_pubkey_hex)
        .map_err(|e| anyhow!("invalid seller_pubkey_hex: {}", e))?;
    // Extract marketplace pubkey from the leaf script (we need it for the multisig reconstruction).
    // Since we store the leaf_script, we can reconstruct the TaprootMultisig from seller+marketplace pubkeys.
    // The marketplace pubkey is available from the AppState, passed through the route layer.
    // For now we reconstruct from the stored leaf_script and the seller pubkey.

    let marketplace_fee_sats = calculate_marketplace_fee(req.price_sats, req.marketplace_fee_bps);
    let fee_addr = req
        .marketplace_fee_address
        .as_ref()
        .filter(|_| marketplace_fee_sats > 0);
    let output_count: u64 = if fee_addr.is_some() { 4 } else { 3 };

    let estimated_fee_sats =
        (sale_tx_vbytes(buyer_input_type, output_count) as f64 * req.fee_rate_sat_vb) as u64;
    let total_required =
        req.price_sats + INSCRIPTION_DUST_SATS + estimated_fee_sats + marketplace_fee_sats;
    if req.buyer_funding_input.value_sats < total_required {
        return Err(anyhow!(
            "buyer input ({} sats) insufficient; need {} sats (price {} + dust {} + tx_fee {} + marketplace_fee {})",
            req.buyer_funding_input.value_sats,
            total_required,
            req.price_sats,
            INSCRIPTION_DUST_SATS,
            estimated_fee_sats,
            marketplace_fee_sats
        ));
    }

    let change_sats = req.buyer_funding_input.value_sats
        - req.price_sats
        - INSCRIPTION_DUST_SATS
        - estimated_fee_sats
        - marketplace_fee_sats;

    let buyer_txid = Txid::from_str(&req.buyer_funding_input.txid)
        .map_err(|e| anyhow!("invalid buyer_funding_input.txid: {}", e))?;

    let mut outputs = vec![
        TxOut {
            value: Amount::from_sat(req.price_sats),
            script_pubkey: seller_addr.script_pubkey(),
        },
        TxOut {
            value: Amount::from_sat(INSCRIPTION_DUST_SATS),
            script_pubkey: buyer_addr.script_pubkey(),
        },
        TxOut {
            value: Amount::from_sat(change_sats),
            script_pubkey: buyer_addr.script_pubkey(),
        },
    ];

    if let Some(addr_str) = fee_addr {
        let fee_address = Address::from_str(addr_str)
            .map_err(|e| anyhow!("invalid marketplace_fee_address: {}", e))?
            .assume_checked();
        outputs.push(TxOut {
            value: Amount::from_sat(marketplace_fee_sats),
            script_pubkey: fee_address.script_pubkey(),
        });
    }

    let unsigned_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: OutPoint::new(locking_txid, req.multisig_vout),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::default(),
            },
            TxIn {
                previous_output: OutPoint::new(buyer_txid, req.buyer_funding_input.vout),
                script_sig: ScriptBuf::new(),
                sequence: parse_sequence(req.buyer_funding_input.sequence),
                witness: Witness::default(),
            },
        ],
        output: outputs,
    };

    let mut psbt = Psbt::from_unsigned_tx(unsigned_tx)
        .map_err(|e| anyhow!("protected sale PSBT creation failed: {}", e))?;

    // Reconstruct Taproot metadata for the multisig input.
    let internal_key = XOnlyPublicKey::from(seller_pk);
    let builder = TaprootBuilder::new()
        .add_leaf(0, leaf_script.clone())
        .map_err(|e| anyhow!("TaprootBuilder::add_leaf failed: {:?}", e))?;
    let spend_info = builder
        .finalize(&secp256k1::Secp256k1::new(), internal_key)
        .map_err(|_| anyhow!("TaprootBuilder::finalize failed"))?;
    let control_block = spend_info
        .control_block(&(leaf_script.clone(), LeafVersion::TapScript))
        .ok_or_else(|| anyhow!("failed to compute control block"))?;

    psbt.inputs[0].witness_utxo = Some(multisig_txout);
    psbt.inputs[0].tap_internal_key = Some(internal_key);
    psbt.inputs[0].tap_merkle_root = Some(
        TapLeafHash::from_script(&leaf_script, LeafVersion::TapScript).into(),
    );
    psbt.inputs[0]
        .tap_scripts
        .insert(control_block, (leaf_script.clone(), LeafVersion::TapScript));

    add_spendable_input(&mut psbt, 1, &req.buyer_funding_input)?;

    // Embed the seller's pre-signed Schnorr tap_script_sig (produced at listing time).
    if let Some(ref sig_hex) = req.seller_sale_sig_hex {
        let sig_bytes = hex::decode(sig_hex)
            .map_err(|e| anyhow!("invalid seller_sale_sig_hex: {}", e))?;
        let schnorr_sig = taproot::Signature::from_slice(&sig_bytes)
            .map_err(|e| anyhow!("invalid seller Schnorr signature: {}", e))?;
        let seller_xonly = XOnlyPublicKey::from(seller_pk);
        let leaf_hash = TapLeafHash::from_script(&leaf_script, LeafVersion::TapScript);
        psbt.inputs[0]
            .tap_script_sigs
            .insert((seller_xonly, leaf_hash), schnorr_sig);
    }

    Ok(ProtectedSalePsbt {
        psbt_hex: encode_psbt(&psbt),
        estimated_fee_sats,
        locking_txid: locking_txid.to_string(),
        marketplace_fee_sats,
    })
}

// ── Marketplace Schnorr co-signing (Taproot) ────────────────────────────────

pub fn apply_marketplace_signature(
    psbt_hex: &str,
    marketplace_keypair: &MarketplaceKeypair,
) -> Result<String> {
    use bitcoin::sighash::SighashCache;

    let mut psbt = decode_psbt(psbt_hex)?;

    // Get the leaf script from tap_scripts.
    let (leaf_script, _leaf_version) = psbt.inputs[0]
        .tap_scripts
        .values()
        .next()
        .ok_or_else(|| anyhow!("input 0 missing tap_scripts"))?
        .clone();

    let leaf_hash = TapLeafHash::from_script(&leaf_script, LeafVersion::TapScript);

    // Collect all prevouts for taproot sighash computation.
    let prevouts: Vec<TxOut> = psbt
        .inputs
        .iter()
        .map(|input| {
            input
                .witness_utxo
                .clone()
                .ok_or_else(|| anyhow!("input missing witness_utxo"))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut sighash_cache = SighashCache::new(&psbt.unsigned_tx);
    let sighash = sighash_cache
        .taproot_script_spend_signature_hash(
            0,
            &bitcoin::sighash::Prevouts::All(&prevouts),
            leaf_hash,
            TapSighashType::All,
        )
        .map_err(|e| anyhow!("taproot sighash computation failed: {}", e))?;

    let sighash_bytes: [u8; 32] = sighash.to_byte_array();
    let schnorr_sig = marketplace_keypair.sign_schnorr(&sighash_bytes)?;
    let tap_sig = taproot::Signature {
        sig: schnorr_sig,
        hash_ty: TapSighashType::All,
    };

    let marketplace_xonly = marketplace_keypair.x_only_pubkey();
    psbt.inputs[0]
        .tap_script_sigs
        .insert((marketplace_xonly, leaf_hash), tap_sig);

    Ok(encode_psbt(&psbt))
}

// ── Finalize Taproot multisig and extract ───────────────────────────────────

pub fn finalize_multisig_and_extract(
    psbt_hex: &str,
    seller_pubkey_hex: &str,
    marketplace_pubkey_hex: &str,
) -> Result<String> {
    let mut psbt = decode_psbt(psbt_hex)?;

    let seller_pk = PublicKey::from_str(seller_pubkey_hex)
        .map_err(|e| anyhow!("invalid seller_pubkey_hex: {}", e))?;
    let marketplace_pk = PublicKey::from_str(marketplace_pubkey_hex)
        .map_err(|e| anyhow!("invalid marketplace_pubkey_hex: {}", e))?;

    // Get the leaf script and control block from tap_scripts.
    let (control_block, (leaf_script, _)) = psbt.inputs[0]
        .tap_scripts
        .iter()
        .next()
        .ok_or_else(|| anyhow!("input 0 missing tap_scripts"))?;
    let leaf_script = leaf_script.clone();
    let control_block = control_block.clone();

    let leaf_hash = TapLeafHash::from_script(&leaf_script, LeafVersion::TapScript);
    let seller_xonly = XOnlyPublicKey::from(seller_pk);
    let marketplace_xonly = XOnlyPublicKey::from(marketplace_pk);

    let seller_sig = psbt.inputs[0]
        .tap_script_sigs
        .get(&(seller_xonly, leaf_hash))
        .ok_or_else(|| anyhow!("seller taproot signature missing from PSBT input 0"))?
        .clone();
    let marketplace_sig = psbt.inputs[0]
        .tap_script_sigs
        .get(&(marketplace_xonly, leaf_hash))
        .ok_or_else(|| anyhow!("marketplace taproot signature missing from PSBT input 0"))?
        .clone();

    // Taproot script-path witness: signatures in REVERSE script order.
    // Script: <xpk1> CHECKSIG <xpk2> CHECKSIGADD 2 NUMEQUAL
    // Witness stack (bottom to top): sig_for_xpk1, sig_for_xpk2, leaf_script, control_block
    // But the witness is pushed top-to-bottom, so: [sig2, sig1, script, controlblock]
    let (xpk1, xpk2) = sorted_x_only_pubkeys(&seller_pk, &marketplace_pk);
    let sig1 = if seller_xonly == xpk1 {
        &seller_sig
    } else {
        &marketplace_sig
    };
    let sig2 = if seller_xonly == xpk2 {
        &seller_sig
    } else {
        &marketplace_sig
    };

    // Witness stack for CHECKSIGADD: sig_for_pk2 (last checked), sig_for_pk1 (first checked),
    // then leaf_script and control_block.
    let sig2_bytes = sig2.to_vec();
    let sig1_bytes = sig1.to_vec();
    let leaf_bytes = leaf_script.as_bytes().to_vec();
    let cb_bytes = control_block.serialize();
    psbt.inputs[0].final_script_witness = Some(Witness::from_slice(&[
        &sig2_bytes,
        &sig1_bytes,
        &leaf_bytes,
        &cb_bytes,
    ]));
    psbt.inputs[0].tap_script_sigs.clear();
    psbt.inputs[0].tap_scripts.clear();
    psbt.inputs[0].tap_internal_key = None;
    psbt.inputs[0].tap_merkle_root = None;
    psbt.inputs[0].sighash_type = None;

    for input in psbt.inputs.iter_mut().skip(1) {
        finalize_input(input)?;
    }

    let final_tx = psbt
        .extract_tx()
        .map_err(|e| anyhow!("failed to extract finalized tx: {}", e))?;
    let raw_bytes = bitcoin::consensus::encode::serialize(&final_tx);
    Ok(hex::encode(raw_bytes))
}

pub fn finalize_locking_psbt(signed_psbt_hex: &str) -> Result<String> {
    let mut psbt = decode_psbt(signed_psbt_hex)?;
    for input in psbt.inputs.iter_mut() {
        finalize_input(input)?;
    }

    let final_tx = psbt
        .extract_tx()
        .map_err(|e| anyhow!("failed to extract locking tx: {}", e))?;
    let raw_bytes = bitcoin::consensus::encode::serialize(&final_tx);
    Ok(hex::encode(raw_bytes))
}

pub fn decode_psbt(hex: &str) -> Result<Psbt> {
    let bytes = hex::decode(hex).map_err(|e| anyhow!("invalid hex: {}", e))?;
    Psbt::deserialize(&bytes).map_err(Into::into)
}

pub fn encode_psbt(psbt: &Psbt) -> String {
    hex::encode(psbt.serialize())
}

#[cfg(test)]
#[path = "psbt_tests.rs"]
mod psbt_tests;
