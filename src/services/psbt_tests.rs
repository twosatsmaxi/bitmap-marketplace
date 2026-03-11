/// Unit tests for the PSBT service.
/// These run without any network, DB, or wallet — pure construction logic only.
use super::{
    build_buy_psbt, build_listing_psbt, decode_psbt, encode_psbt, BuyRequest, ListingRequest,
};
use bitcoin::{
    absolute::LockTime, psbt::Psbt, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn,
    TxOut, Txid, Witness,
};
use std::str::FromStr;

/// A valid-looking but fake txid (all zeros except last byte).
const FAKE_TXID: &str = "0000000000000000000000000000000000000000000000000000000000000001";
/// A valid regtest/mainnet bech32 address (P2WPKH on mainnet).
const SELLER_ADDR: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
const BUYER_ADDR: &str = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";

// ---------------------------------------------------------------------------
// build_listing_psbt
// ---------------------------------------------------------------------------

#[test]
fn listing_psbt_roundtrip() {
    let req = ListingRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 1_000_000,
    };
    let result = build_listing_psbt(&req).unwrap();

    // Round-trip through decode → encode should be stable.
    let psbt = decode_psbt(&result.psbt_hex).unwrap();
    let rehex = encode_psbt(&psbt);
    assert_eq!(result.psbt_hex, rehex);
}

#[test]
fn listing_psbt_has_one_input_one_output() {
    let req = ListingRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 2,
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 500_000,
    };
    let result = build_listing_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    assert_eq!(psbt.unsigned_tx.input.len(), 1, "exactly one input");
    assert_eq!(psbt.unsigned_tx.output.len(), 1, "exactly one output");
}

#[test]
fn listing_psbt_output_value_matches_price() {
    let price = 2_000_000u64;
    let req = ListingRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        seller_address: SELLER_ADDR.to_string(),
        price_sats: price,
    };
    let result = build_listing_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    let output_sats = psbt.unsigned_tx.output[0].value.to_sat();
    assert_eq!(output_sats, price, "output value must equal price_sats");
}

#[test]
fn listing_psbt_input_references_correct_outpoint() {
    let req = ListingRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 3,
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 100_000,
    };
    let result = build_listing_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    let input = &psbt.unsigned_tx.input[0];
    assert_eq!(
        input.previous_output.txid.to_string(),
        FAKE_TXID,
        "input txid must match"
    );
    assert_eq!(input.previous_output.vout, 3, "input vout must match");
}

#[test]
fn listing_psbt_sighash_type_is_single_acp() {
    let req = ListingRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 100_000,
    };
    let result = build_listing_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    let sighash_type = psbt.inputs[0]
        .sighash_type
        .expect("sighash_type must be set on seller input");

    let ecdsa = sighash_type
        .ecdsa_hash_ty()
        .expect("must be a valid ECDSA sighash type");

    assert_eq!(
        ecdsa,
        bitcoin::EcdsaSighashType::SinglePlusAnyoneCanPay,
        "seller input must use SIGHASH_SINGLE|ANYONECANPAY"
    );
}

#[test]
fn listing_psbt_invalid_txid_returns_error() {
    let req = ListingRequest {
        inscription_txid: "not-a-txid".to_string(),
        inscription_vout: 0,
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 100_000,
    };
    assert!(build_listing_psbt(&req).is_err());
}

#[test]
fn listing_psbt_invalid_address_returns_error() {
    let req = ListingRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        seller_address: "not-an-address".to_string(),
        price_sats: 100_000,
    };
    assert!(build_listing_psbt(&req).is_err());
}

// ---------------------------------------------------------------------------
// build_buy_psbt
// ---------------------------------------------------------------------------

/// Build a minimal seller PSBT hex to use as input for buy tests.
fn make_seller_psbt_hex(price_sats: u64) -> String {
    let req = ListingRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        seller_address: SELLER_ADDR.to_string(),
        price_sats,
    };
    build_listing_psbt(&req).unwrap().psbt_hex
}

#[test]
fn buy_psbt_has_two_inputs_three_outputs() {
    let price = 500_000u64;
    let seller_hex = make_seller_psbt_hex(price);
    let req = BuyRequest {
        seller_psbt_hex: seller_hex,
        buyer_address: BUYER_ADDR.to_string(),
        buyer_utxo_txid: FAKE_TXID.to_string(),
        buyer_utxo_vout: 1,
        buyer_utxo_amount_sats: 1_000_000,
        fee_rate_sat_vb: 10.0,
    };
    let result = build_buy_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    assert_eq!(psbt.unsigned_tx.input.len(), 2, "2 inputs: seller + buyer");
    assert_eq!(
        psbt.unsigned_tx.output.len(),
        3,
        "3 outputs: payment + dust + change"
    );
}

#[test]
fn buy_psbt_output0_is_price_to_seller() {
    let price = 500_000u64;
    let seller_hex = make_seller_psbt_hex(price);
    let req = BuyRequest {
        seller_psbt_hex: seller_hex,
        buyer_address: BUYER_ADDR.to_string(),
        buyer_utxo_txid: FAKE_TXID.to_string(),
        buyer_utxo_vout: 1,
        buyer_utxo_amount_sats: 1_000_000,
        fee_rate_sat_vb: 10.0,
    };
    let result = build_buy_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    assert_eq!(
        psbt.unsigned_tx.output[0].value.to_sat(),
        price,
        "output 0 must equal the listing price"
    );
}

#[test]
fn buy_psbt_output1_is_inscription_dust() {
    let price = 500_000u64;
    let seller_hex = make_seller_psbt_hex(price);
    let req = BuyRequest {
        seller_psbt_hex: seller_hex,
        buyer_address: BUYER_ADDR.to_string(),
        buyer_utxo_txid: FAKE_TXID.to_string(),
        buyer_utxo_vout: 1,
        buyer_utxo_amount_sats: 1_000_000,
        fee_rate_sat_vb: 10.0,
    };
    let result = build_buy_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    assert_eq!(
        psbt.unsigned_tx.output[1].value.to_sat(),
        546,
        "output 1 must be the 546-sat inscription dust"
    );
}

#[test]
fn buy_psbt_change_output_is_correct() {
    let price = 500_000u64;
    let buyer_utxo = 1_000_000u64;
    let fee_rate = 10.0f64;
    let seller_hex = make_seller_psbt_hex(price);

    let req = BuyRequest {
        seller_psbt_hex: seller_hex,
        buyer_address: BUYER_ADDR.to_string(),
        buyer_utxo_txid: FAKE_TXID.to_string(),
        buyer_utxo_vout: 1,
        buyer_utxo_amount_sats: buyer_utxo,
        fee_rate_sat_vb: fee_rate,
    };
    let result = build_buy_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    // Manually compute the expected change.
    // estimate_tx_vbytes(2, 3) = 10 + 41*2 + 31*3 = 10 + 82 + 93 = 185
    let expected_fee = (185.0 * fee_rate) as u64; // 1850
    let expected_change = buyer_utxo - price - 546 - expected_fee;

    assert_eq!(
        psbt.unsigned_tx.output[2].value.to_sat(),
        expected_change,
        "output 2 must be the correct change"
    );
    assert_eq!(
        result.estimated_fee_sats, expected_fee,
        "reported fee must match"
    );
}

#[test]
fn buy_psbt_insufficient_funds_returns_error() {
    let price = 500_000u64;
    let seller_hex = make_seller_psbt_hex(price);
    let req = BuyRequest {
        seller_psbt_hex: seller_hex,
        buyer_address: BUYER_ADDR.to_string(),
        buyer_utxo_txid: FAKE_TXID.to_string(),
        buyer_utxo_vout: 1,
        buyer_utxo_amount_sats: 400_000, // less than price alone
        fee_rate_sat_vb: 10.0,
    };
    assert!(
        build_buy_psbt(&req).is_err(),
        "insufficient funds must return error"
    );
}

#[test]
fn buy_psbt_invalid_seller_psbt_returns_error() {
    let req = BuyRequest {
        seller_psbt_hex: "deadbeef".to_string(),
        buyer_address: BUYER_ADDR.to_string(),
        buyer_utxo_txid: FAKE_TXID.to_string(),
        buyer_utxo_vout: 0,
        buyer_utxo_amount_sats: 1_000_000,
        fee_rate_sat_vb: 10.0,
    };
    assert!(build_buy_psbt(&req).is_err());
}

#[test]
fn buy_psbt_roundtrip_decode_encode() {
    let price = 300_000u64;
    let seller_hex = make_seller_psbt_hex(price);
    let req = BuyRequest {
        seller_psbt_hex: seller_hex,
        buyer_address: BUYER_ADDR.to_string(),
        buyer_utxo_txid: FAKE_TXID.to_string(),
        buyer_utxo_vout: 0,
        buyer_utxo_amount_sats: 1_000_000,
        fee_rate_sat_vb: 5.0,
    };
    let result = build_buy_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();
    let rehex = encode_psbt(&psbt);
    assert_eq!(result.psbt_hex, rehex);
}

// ---------------------------------------------------------------------------
// decode_psbt / encode_psbt helpers
// ---------------------------------------------------------------------------

#[test]
fn decode_invalid_hex_returns_error() {
    assert!(decode_psbt("gg").is_err());
}

#[test]
fn decode_invalid_psbt_bytes_returns_error() {
    // Valid hex but not a PSBT.
    assert!(decode_psbt("deadbeef").is_err());
}

// ---------------------------------------------------------------------------
// Mempool protection: build_locking_psbt
// ---------------------------------------------------------------------------

use super::{
    build_locking_psbt, build_multisig_redeem_script, p2wsh_address, LockingPsbtRequest,
    DEFAULT_LOCKING_TX_FEE_SATS, MIN_SELF_FUNDED,
};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use bitcoin::Network;

/// Generate a deterministic secp256k1 keypair from a seed byte.
fn make_keypair(seed: u8) -> (SecretKey, bitcoin::secp256k1::PublicKey) {
    let secp = Secp256k1::new();
    let mut key_bytes = [0u8; 32];
    key_bytes[31] = seed;
    let sk = SecretKey::from_slice(&key_bytes).unwrap();
    let pk = bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &sk);
    (sk, pk)
}

fn seller_pk_hex() -> String {
    let (_, pk) = make_keypair(1);
    hex::encode(pk.serialize())
}

fn marketplace_pk_hex() -> String {
    let (_, pk) = make_keypair(2);
    hex::encode(pk.serialize())
}

#[test]
fn locking_psbt_roundtrip() {
    let req = LockingPsbtRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        inscription_amount_sats: MIN_SELF_FUNDED + 100,
        seller_pubkey_hex: seller_pk_hex(),
        marketplace_pubkey_hex: marketplace_pk_hex(),
        network: Network::Bitcoin,
        fee_rate_sat_vb: None,
        gas_txid: None,
        gas_vout: None,
        gas_amount_sats: None,
    };
    let result = build_locking_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();
    let rehex = encode_psbt(&psbt);
    assert_eq!(result.psbt_hex, rehex);
}

#[test]
fn locking_psbt_has_one_input_one_output() {
    let req = LockingPsbtRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        inscription_amount_sats: 1000,
        seller_pubkey_hex: seller_pk_hex(),
        marketplace_pubkey_hex: marketplace_pk_hex(),
        network: Network::Bitcoin,
        fee_rate_sat_vb: None,
        gas_txid: None,
        gas_vout: None,
        gas_amount_sats: None,
    };
    let result = build_locking_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();
    assert_eq!(psbt.unsigned_tx.input.len(), 1);
    assert_eq!(psbt.unsigned_tx.output.len(), 1);
}

#[test]
fn locking_psbt_output_is_multisig_address() {
    let req = LockingPsbtRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        inscription_amount_sats: 1000,
        seller_pubkey_hex: seller_pk_hex(),
        marketplace_pubkey_hex: marketplace_pk_hex(),
        network: Network::Bitcoin,
        fee_rate_sat_vb: None,
        gas_txid: None,
        gas_vout: None,
        gas_amount_sats: None,
    };
    let result = build_locking_psbt(&req).unwrap();
    assert!(!result.multisig_address.is_empty());
    assert!(!result.multisig_script_hex.is_empty());
    // Multisig address must be a P2WSH (bech32, starts with bc1q on mainnet, 62 chars).
    assert!(
        result.multisig_address.starts_with("bc1q"),
        "must be P2WSH bech32"
    );
}

#[test]
fn locking_psbt_output_value_deducts_fee() {
    let inscription_amount = 1000u64;
    let req = LockingPsbtRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        inscription_amount_sats: inscription_amount,
        seller_pubkey_hex: seller_pk_hex(),
        marketplace_pubkey_hex: marketplace_pk_hex(),
        network: Network::Bitcoin,
        fee_rate_sat_vb: None,
        gas_txid: None,
        gas_vout: None,
        gas_amount_sats: None,
    };
    let result = build_locking_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();
    let out_value = psbt.unsigned_tx.output[0].value.to_sat();
    assert_eq!(out_value, inscription_amount - DEFAULT_LOCKING_TX_FEE_SATS);
}

#[test]
fn locking_psbt_below_min_self_funded_returns_error() {
    let req = LockingPsbtRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        inscription_amount_sats: MIN_SELF_FUNDED - 1, // too small
        seller_pubkey_hex: seller_pk_hex(),
        marketplace_pubkey_hex: marketplace_pk_hex(),
        network: Network::Bitcoin,
        fee_rate_sat_vb: None,
        gas_txid: None,
        gas_vout: None,
        gas_amount_sats: None,
    };
    assert!(build_locking_psbt(&req).is_err());
}

#[test]
fn locking_psbt_with_gas_utxo_has_two_inputs() {
    let req = LockingPsbtRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        inscription_amount_sats: 546,
        seller_pubkey_hex: seller_pk_hex(),
        marketplace_pubkey_hex: marketplace_pk_hex(),
        network: Network::Bitcoin,
        fee_rate_sat_vb: None,
        gas_txid: Some(FAKE_TXID.to_string()),
        gas_vout: Some(1),
        gas_amount_sats: Some(2000),
    };
    let result = build_locking_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();
    assert_eq!(
        psbt.unsigned_tx.input.len(),
        2,
        "gas input adds a second input"
    );
}

#[test]
fn bip67_sort_is_deterministic() {
    let (_, pk1) = make_keypair(1);
    let (_, pk2) = make_keypair(2);
    // Redeem script built with pk1+pk2 and pk2+pk1 in LockingPsbtRequest should produce same address.
    let script_a = build_multisig_redeem_script(&pk1, &pk2);
    let script_b = build_multisig_redeem_script(&pk2, &pk1);
    assert_eq!(
        script_a, script_b,
        "BIP-67 sort must produce same script regardless of key order"
    );
    let addr_a = p2wsh_address(&script_a, Network::Bitcoin);
    let addr_b = p2wsh_address(&script_b, Network::Bitcoin);
    assert_eq!(addr_a, addr_b, "same script must produce same address");
}

#[test]
fn locking_psbt_invalid_seller_pubkey_returns_error() {
    let req = LockingPsbtRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        inscription_amount_sats: 1000,
        seller_pubkey_hex: "not-a-pubkey".to_string(),
        marketplace_pubkey_hex: marketplace_pk_hex(),
        network: Network::Bitcoin,
        fee_rate_sat_vb: None,
        gas_txid: None,
        gas_vout: None,
        gas_amount_sats: None,
    };
    assert!(build_locking_psbt(&req).is_err());
}
