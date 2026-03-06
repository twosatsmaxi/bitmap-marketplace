/// Unit tests for the PSBT service.
/// These run without any network, DB, or wallet — pure construction logic only.
use super::{
    build_buy_psbt, build_listing_psbt, decode_psbt, encode_psbt, BuyRequest, ListingRequest,
};
use bitcoin::{
    absolute::LockTime, psbt::Psbt, Amount, OutPoint, ScriptBuf, Sequence, Transaction,
    TxIn, TxOut, Txid, Witness,
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
    assert_eq!(psbt.unsigned_tx.output.len(), 3, "3 outputs: payment + dust + change");
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
    assert!(build_buy_psbt(&req).is_err(), "insufficient funds must return error");
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
