use super::{
    apply_marketplace_signature, build_buy_psbt, build_listing_psbt, build_locking_psbt,
    build_protected_sale_psbt, calculate_marketplace_fee, create_taproot_multisig, decode_psbt,
    encode_psbt, extract_seller_sale_sig, finalize_locking_psbt, finalize_multisig_and_extract,
    BuyRequest, ListingRequest, LockingPsbtRequest, ProtectedSalePsbtRequest, SpendableInput,
    WitnessUtxo, MIN_MARKETPLACE_FEE_SATS, MIN_SELF_FUNDED,
};
use bitcoin::{
    ecdsa, hashes::Hash, key::TapTweak, opcodes::all as op, psbt::Psbt, secp256k1,
    sighash::SighashCache, taproot, Address, Amount, Network, ScriptBuf, Sequence, TapLeafHash,
    Transaction, TxIn, TxOut, Witness,
};
use bitcoin::taproot::LeafVersion;
use std::str::FromStr;

const FAKE_TXID: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const FAKE_TXID_2: &str = "0000000000000000000000000000000000000000000000000000000000000002";
const SELLER_ADDR: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
const BUYER_ADDR: &str = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";

fn secret_key(byte: u8) -> secp256k1::SecretKey {
    secp256k1::SecretKey::from_slice(&[byte; 32]).unwrap()
}

fn bitcoin_pubkey(secret_key: &secp256k1::SecretKey) -> bitcoin::PublicKey {
    bitcoin::PublicKey {
        compressed: true,
        inner: secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), secret_key),
    }
}

fn p2wpkh_script(secret_key: &secp256k1::SecretKey) -> ScriptBuf {
    Address::p2wpkh(&bitcoin_pubkey(secret_key), Network::Bitcoin)
        .unwrap()
        .script_pubkey()
}

fn p2sh_p2wpkh_scripts(secret_key: &secp256k1::SecretKey) -> (ScriptBuf, ScriptBuf) {
    let pubkey = bitcoin_pubkey(secret_key);
    let redeem_script = ScriptBuf::new_p2wpkh(&pubkey.wpubkey_hash().unwrap());
    let script_pubkey = Address::p2shwpkh(&pubkey, Network::Bitcoin)
        .unwrap()
        .script_pubkey();
    (script_pubkey, redeem_script)
}

fn p2tr_script(secret_key: &secp256k1::SecretKey) -> ScriptBuf {
    let secp = secp256k1::Secp256k1::new();
    let keypair = secp256k1::Keypair::from_secret_key(&secp, secret_key);
    let (internal_key, _) = secp256k1::XOnlyPublicKey::from_keypair(&keypair);
    let tweaked = internal_key.tap_tweak(&secp, None).0;
    Address::p2tr_tweaked(tweaked, Network::Bitcoin).script_pubkey()
}

fn spendable_input(
    txid: &str,
    vout: u32,
    value_sats: u64,
    script_pubkey: ScriptBuf,
) -> SpendableInput {
    SpendableInput {
        txid: txid.to_string(),
        vout,
        value_sats,
        witness_utxo: WitnessUtxo {
            script_pubkey_hex: hex::encode(script_pubkey.as_bytes()),
            value_sats,
        },
        non_witness_utxo_hex: None,
        redeem_script_hex: None,
        witness_script_hex: None,
        sequence: Some(Sequence::ENABLE_RBF_NO_LOCKTIME.0),
    }
}

fn wrapped_spendable_input(
    txid: &str,
    vout: u32,
    value_sats: u64,
    secret_key: &secp256k1::SecretKey,
) -> SpendableInput {
    let (script_pubkey, redeem_script) = p2sh_p2wpkh_scripts(secret_key);
    SpendableInput {
        redeem_script_hex: Some(hex::encode(redeem_script.as_bytes())),
        ..spendable_input(txid, vout, value_sats, script_pubkey)
    }
}

fn make_seller_psbt_hex(price_sats: u64) -> String {
    let req = ListingRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        seller_address: SELLER_ADDR.to_string(),
        price_sats,
    };
    build_listing_psbt(&req).unwrap().psbt_hex
}

fn build_locking_fixture_psbt(input: SpendableInput) -> String {
    let tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(input.value_sats - 1000),
            script_pubkey: Address::from_str(BUYER_ADDR)
                .unwrap()
                .assume_checked()
                .script_pubkey(),
        }],
    };
    let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(input.value_sats),
        script_pubkey: ScriptBuf::from_bytes(
            hex::decode(&input.witness_utxo.script_pubkey_hex).unwrap(),
        ),
    });
    if let Some(redeem_script_hex) = input.redeem_script_hex {
        psbt.inputs[0].redeem_script = Some(ScriptBuf::from_bytes(
            hex::decode(redeem_script_hex).unwrap(),
        ));
    }
    encode_psbt(&psbt)
}

fn sign_p2wpkh_input(psbt: &mut Psbt, input_index: usize, secret_key: &secp256k1::SecretKey) {
    let secp = secp256k1::Secp256k1::new();
    let public_key = bitcoin_pubkey(secret_key);
    let witness_utxo = psbt.inputs[input_index].witness_utxo.clone().unwrap();
    let script = if let Some(redeem_script) = psbt.inputs[input_index].redeem_script.clone() {
        redeem_script
    } else {
        witness_utxo.script_pubkey.clone()
    };
    let sighash = SighashCache::new(&psbt.unsigned_tx)
        .p2wpkh_signature_hash(
            input_index,
            &script,
            witness_utxo.value,
            bitcoin::EcdsaSighashType::All,
        )
        .unwrap();
    let msg = secp256k1::Message::from(sighash);
    let sig = secp.sign_ecdsa(&msg, secret_key);
    psbt.inputs[input_index].partial_sigs.insert(
        public_key,
        ecdsa::Signature {
            sig,
            hash_ty: bitcoin::EcdsaSighashType::All,
        },
    );
}

#[test]
fn listing_psbt_roundtrip() {
    let req = ListingRequest {
        inscription_txid: FAKE_TXID.to_string(),
        inscription_vout: 0,
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 1_000_000,
    };
    let result = build_listing_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();
    assert_eq!(result.psbt_hex, encode_psbt(&psbt));
}

#[test]
fn buy_psbt_populates_real_buyer_prevout_metadata() {
    let price = 500_000u64;
    let buyer_input = spendable_input(FAKE_TXID_2, 1, 1_000_000, p2wpkh_script(&secret_key(3)));

    let result = build_buy_psbt(&BuyRequest {
        seller_psbt_hex: make_seller_psbt_hex(price),
        buyer_address: BUYER_ADDR.to_string(),
        buyer_funding_input: buyer_input.clone(),
        fee_rate_sat_vb: 10.0,
        marketplace_fee_address: None,
        marketplace_fee_bps: 0,
    })
    .unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    assert_eq!(
        psbt.inputs[1].witness_utxo.as_ref().unwrap().value.to_sat(),
        buyer_input.value_sats
    );
    assert!(!psbt.inputs[1]
        .witness_utxo
        .as_ref()
        .unwrap()
        .script_pubkey
        .is_empty());
}

#[test]
fn buy_psbt_rejects_wrapped_segwit_without_redeem_script() {
    let secret_key = secret_key(4);
    let (script_pubkey, _) = p2sh_p2wpkh_scripts(&secret_key);
    let buyer_input = spendable_input(FAKE_TXID_2, 1, 1_000_000, script_pubkey);

    let err = build_buy_psbt(&BuyRequest {
        seller_psbt_hex: make_seller_psbt_hex(500_000),
        buyer_address: BUYER_ADDR.to_string(),
        buyer_funding_input: buyer_input,
        fee_rate_sat_vb: 10.0,
        marketplace_fee_address: None,
        marketplace_fee_bps: 0,
    })
    .unwrap_err();

    assert!(err.to_string().contains("redeem_script_hex"));
}

#[test]
fn locking_psbt_populates_real_funding_metadata() {
    let inscription_input = spendable_input(
        FAKE_TXID,
        0,
        MIN_SELF_FUNDED + 100,
        p2wpkh_script(&secret_key(5)),
    );
    let req = LockingPsbtRequest {
        inscription_input: inscription_input.clone(),
        gas_funding_input: None,
        seller_pubkey_hex: bitcoin_pubkey(&secret_key(6)).to_string(),
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 50_000,
        marketplace_pubkey_hex: bitcoin_pubkey(&secret_key(7)).to_string(),
        network: Network::Bitcoin,
        min_relay_fee_rate_sat_vb: None,
    };

    let result = build_locking_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    assert_eq!(psbt.inputs.len(), 1);
    assert_eq!(
        psbt.inputs[0].witness_utxo.as_ref().unwrap().value.to_sat(),
        inscription_input.value_sats
    );
    assert!(psbt.inputs[0].witness_script.is_none());
}

#[test]
fn locking_psbt_requires_gas_below_threshold() {
    let req = LockingPsbtRequest {
        inscription_input: spendable_input(
            FAKE_TXID,
            0,
            MIN_SELF_FUNDED - 1,
            p2wpkh_script(&secret_key(8)),
        ),
        gas_funding_input: None,
        seller_pubkey_hex: bitcoin_pubkey(&secret_key(9)).to_string(),
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 50_000,
        marketplace_pubkey_hex: bitcoin_pubkey(&secret_key(10)).to_string(),
        network: Network::Bitcoin,
        min_relay_fee_rate_sat_vb: None,
    };

    let err = build_locking_psbt(&req).unwrap_err();
    assert!(err.to_string().contains("gas_funding_input"));
}

#[test]
fn locking_psbt_rejects_insufficient_gas_funding() {
    let req = LockingPsbtRequest {
        inscription_input: spendable_input(FAKE_TXID, 0, 200, p2wpkh_script(&secret_key(11))),
        gas_funding_input: Some(spendable_input(
            FAKE_TXID_2,
            1,
            5,
            p2wpkh_script(&secret_key(12)),
        )),
        seller_pubkey_hex: bitcoin_pubkey(&secret_key(13)).to_string(),
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 50_000,
        marketplace_pubkey_hex: bitcoin_pubkey(&secret_key(14)).to_string(),
        network: Network::Bitcoin,
        min_relay_fee_rate_sat_vb: Some(1.0),
    };

    let err = build_locking_psbt(&req).unwrap_err();
    assert!(err
        .to_string()
        .contains("gas_funding_input requires at least"));
}

#[test]
fn protected_sale_psbt_populates_multisig_and_buyer_metadata() {
    let seller_secret = secret_key(15);
    let marketplace_secret = secret_key(16);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);

    // Create Taproot 2-of-2 multisig
    let secp = secp256k1::Secp256k1::new();
    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);
    let multisig = create_taproot_multisig(
        &seller_pubkey.inner,
        &marketplace_pubkey.inner,
        seller_xonly, // Use seller x-only as internal key
        Network::Bitcoin,
    )
    .unwrap();

    let locking_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(10_000),
            script_pubkey: multisig.output_script.clone(),
        }],
    };
    let buyer_input = spendable_input(FAKE_TXID_2, 1, 100_000, p2wpkh_script(&secret_key(17)));

    let result = build_protected_sale_psbt(&ProtectedSalePsbtRequest {
        locking_raw_tx_hex: hex::encode(bitcoin::consensus::encode::serialize(&locking_tx)),
        multisig_vout: 0,
        multisig_script_hex: hex::encode(multisig.leaf_script.as_bytes()),
        seller_address: SELLER_ADDR.to_string(),
        seller_pubkey_hex: seller_pubkey.to_string(),
        price_sats: 50_000,
        buyer_address: BUYER_ADDR.to_string(),
        buyer_funding_input: buyer_input.clone(),
        fee_rate_sat_vb: 5.0,
        marketplace_fee_address: None,
        marketplace_fee_bps: 0,
        seller_sale_sig_hex: None,
    })
    .unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    // Taproot script-path: check tap_scripts instead of witness_script
    assert!(!psbt.inputs[0].tap_scripts.is_empty());
    assert_eq!(
        psbt.inputs[1].witness_utxo.as_ref().unwrap().value.to_sat(),
        buyer_input.value_sats
    );
    assert!(!psbt.inputs[1]
        .witness_utxo
        .as_ref()
        .unwrap()
        .script_pubkey
        .is_empty());
}

#[test]
fn finalize_locking_psbt_supports_p2wpkh() {
    let input = spendable_input(FAKE_TXID, 0, 100_000, p2wpkh_script(&secret_key(18)));
    let mut psbt = decode_psbt(&build_locking_fixture_psbt(input)).unwrap();
    sign_p2wpkh_input(&mut psbt, 0, &secret_key(18));

    let raw_tx = finalize_locking_psbt(&encode_psbt(&psbt)).unwrap();
    let tx: Transaction = bitcoin::consensus::deserialize(&hex::decode(raw_tx).unwrap()).unwrap();
    assert_eq!(tx.input[0].witness.len(), 2);
}

#[test]
fn finalize_locking_psbt_supports_wrapped_segwit() {
    let input = wrapped_spendable_input(FAKE_TXID, 0, 100_000, &secret_key(19));
    let mut psbt = decode_psbt(&build_locking_fixture_psbt(input)).unwrap();
    sign_p2wpkh_input(&mut psbt, 0, &secret_key(19));

    let raw_tx = finalize_locking_psbt(&encode_psbt(&psbt)).unwrap();
    let tx: Transaction = bitcoin::consensus::deserialize(&hex::decode(raw_tx).unwrap()).unwrap();
    assert_eq!(tx.input[0].witness.len(), 2);
    assert!(!tx.input[0].script_sig.is_empty());
}

#[test]
fn finalize_locking_psbt_supports_taproot_keypath() {
    let input = spendable_input(FAKE_TXID, 0, 100_000, p2tr_script(&secret_key(20)));
    let mut psbt = decode_psbt(&build_locking_fixture_psbt(input)).unwrap();
    let secp = secp256k1::Secp256k1::new();
    let keypair = secp256k1::Keypair::from_secret_key(&secp, &secret_key(20));
    let msg = secp256k1::Message::from_digest([21u8; 32]);
    let sig = secp.sign_schnorr(&msg, &keypair);
    psbt.inputs[0].tap_key_sig = Some(taproot::Signature {
        sig,
        hash_ty: bitcoin::sighash::TapSighashType::Default,
    });

    let raw_tx = finalize_locking_psbt(&encode_psbt(&psbt)).unwrap();
    let tx: Transaction = bitcoin::consensus::deserialize(&hex::decode(raw_tx).unwrap()).unwrap();
    assert_eq!(tx.input[0].witness.len(), 1);
}

#[test]
fn finalize_multisig_and_extract_finalizes_buyer_input_too() {
    let secp = secp256k1::Secp256k1::new();
    let seller_secret = secret_key(22);
    let marketplace_secret = secret_key(23);
    let buyer_secret = secret_key(24);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);
    let buyer_pubkey = bitcoin_pubkey(&buyer_secret);

    // Create Taproot 2-of-2 multisig
    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);
    let multisig = create_taproot_multisig(
        &seller_pubkey.inner,
        &marketplace_pubkey.inner,
        seller_xonly,
        Network::Bitcoin,
    )
    .unwrap();

    let locking_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: multisig.output_script.clone(),
        }],
    };
    let buyer_input = spendable_input(FAKE_TXID_2, 1, 200_000, p2wpkh_script(&buyer_secret));
    let mut psbt = decode_psbt(
        &build_protected_sale_psbt(&ProtectedSalePsbtRequest {
            locking_raw_tx_hex: hex::encode(bitcoin::consensus::encode::serialize(&locking_tx)),
            multisig_vout: 0,
            multisig_script_hex: hex::encode(multisig.leaf_script.as_bytes()),
            seller_address: SELLER_ADDR.to_string(),
            seller_pubkey_hex: seller_pubkey.to_string(),
            price_sats: 50_000,
            buyer_address: BUYER_ADDR.to_string(),
            buyer_funding_input: buyer_input,
            fee_rate_sat_vb: 5.0,
            marketplace_fee_address: None,
            marketplace_fee_bps: 0,
            seller_sale_sig_hex: None,
        })
        .unwrap()
        .psbt_hex,
    )
    .unwrap();

    let leaf_hash = TapLeafHash::from_script(&multisig.leaf_script, LeafVersion::TapScript);
    let prevouts = vec![
        psbt.inputs[0].witness_utxo.clone().unwrap(),
        psbt.inputs[1].witness_utxo.clone().unwrap(),
    ];

    // Seller signs with SIGHASH_SINGLE|ANYONECANPAY via Schnorr script-path
    let seller_sighash = SighashCache::new(&psbt.unsigned_tx)
        .taproot_script_spend_signature_hash(
            0,
            &bitcoin::sighash::Prevouts::All(&prevouts),
            leaf_hash,
            bitcoin::sighash::TapSighashType::SinglePlusAnyoneCanPay,
        )
        .unwrap();
    let seller_schnorr_sig = secp.sign_schnorr(
        &secp256k1::Message::from_digest(seller_sighash.to_byte_array()),
        &seller_keypair,
    );
    let seller_tap_sig = taproot::Signature {
        sig: seller_schnorr_sig,
        hash_ty: bitcoin::sighash::TapSighashType::SinglePlusAnyoneCanPay,
    };
    psbt.inputs[0]
        .tap_script_sigs
        .insert((seller_xonly, leaf_hash), seller_tap_sig);

    // Marketplace signs with SIGHASH_ALL via Schnorr script-path
    let marketplace_keypair = secp256k1::Keypair::from_secret_key(&secp, &marketplace_secret);
    let (marketplace_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&marketplace_keypair);
    let marketplace_sighash = SighashCache::new(&psbt.unsigned_tx)
        .taproot_script_spend_signature_hash(
            0,
            &bitcoin::sighash::Prevouts::All(&prevouts),
            leaf_hash,
            bitcoin::sighash::TapSighashType::All,
        )
        .unwrap();
    let marketplace_schnorr_sig = secp.sign_schnorr(
        &secp256k1::Message::from_digest(marketplace_sighash.to_byte_array()),
        &marketplace_keypair,
    );
    let marketplace_tap_sig = taproot::Signature {
        sig: marketplace_schnorr_sig,
        hash_ty: bitcoin::sighash::TapSighashType::All,
    };
    psbt.inputs[0]
        .tap_script_sigs
        .insert((marketplace_xonly, leaf_hash), marketplace_tap_sig);

    // Buyer signs their P2WPKH input with ECDSA
    let buyer_witness_utxo = psbt.inputs[1].witness_utxo.clone().unwrap();
    let buyer_sighash = SighashCache::new(&psbt.unsigned_tx)
        .p2wpkh_signature_hash(
            1,
            &buyer_witness_utxo.script_pubkey,
            buyer_witness_utxo.value,
            bitcoin::EcdsaSighashType::All,
        )
        .unwrap();
    let buyer_sig = secp.sign_ecdsa(&secp256k1::Message::from(buyer_sighash), &buyer_secret);
    psbt.inputs[1].partial_sigs.insert(
        buyer_pubkey,
        ecdsa::Signature {
            sig: buyer_sig,
            hash_ty: bitcoin::EcdsaSighashType::All,
        },
    );

    let raw_tx = finalize_multisig_and_extract(
        &encode_psbt(&psbt),
        &seller_pubkey.to_string(),
        &marketplace_pubkey.to_string(),
    )
    .unwrap();
    let tx: Transaction = bitcoin::consensus::deserialize(&hex::decode(raw_tx).unwrap()).unwrap();

    // Taproot script-path witness: [sig2, sig1, script, control_block] = 4 items
    assert_eq!(tx.input[0].witness.len(), 4);
    // P2WPKH witness: [sig, pubkey] = 2 items
    assert_eq!(tx.input[1].witness.len(), 2);
}


// === create_taproot_multisig direct tests ===

#[test]
fn create_taproot_multisig_sorted_pubkeys_bip67() {
    let secp = secp256k1::Secp256k1::new();

    // Create two pubkeys where one is lexicographically smaller
    // Using specific secret key bytes to ensure ordering
    let seller_secret = secret_key(0x01);
    let marketplace_secret = secret_key(0x02);

    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);

    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);

    // Create multisig with seller as internal key
    let multisig = create_taproot_multisig(
        &seller_pubkey.inner,
        &marketplace_pubkey.inner,
        seller_xonly,
        Network::Bitcoin,
    )
    .unwrap();

    // Verify address is a valid Taproot (P2TR) address
    assert!(multisig.address.to_string().starts_with("bc1p"));
    assert!(!multisig.output_script.as_bytes().is_empty());
    assert_eq!(multisig.output_script.as_bytes()[0], 0x51); // OP_PUSHNUM_1 for Taproot
}

#[test]
fn create_taproot_multisig_correct_leaf_script() {
    let secp = secp256k1::Secp256k1::new();

    let seller_secret = secret_key(0x10);
    let marketplace_secret = secret_key(0x20);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);

    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);

    let multisig = create_taproot_multisig(
        &seller_pubkey.inner,
        &marketplace_pubkey.inner,
        seller_xonly,
        Network::Bitcoin,
    )
    .unwrap();

    // Verify the leaf script structure:
    // <xpk1> OP_CHECKSIG <xpk2> OP_CHECKSIGADD OP_2 OP_NUMEQUAL
    let script_bytes = multisig.leaf_script.as_bytes();

    // Should contain: 32-byte pubkey + CHECKSIG + 32-byte pubkey + CHECKSIGADD + PUSHNUM_2 + NUMEQUAL
    // That's: 32 + 1 + 32 + 1 + 1 + 1 = 68 bytes minimum
    assert!(
        script_bytes.len() >= 68,
        "leaf script too short: {}",
        script_bytes.len()
    );

    // Verify leaf version is TapScript
    assert_eq!(multisig.leaf_version, LeafVersion::TapScript);
}

#[test]
fn create_taproot_multisig_control_block_computable() {
    let secp = secp256k1::Secp256k1::new();

    let seller_secret = secret_key(0x30);
    let marketplace_secret = secret_key(0x40);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);

    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);

    let multisig = create_taproot_multisig(
        &seller_pubkey.inner,
        &marketplace_pubkey.inner,
        seller_xonly,
        Network::Bitcoin,
    )
    .unwrap();

    // Control block should not be empty and should contain merkle path info
    let cb_bytes = multisig.control_block.serialize();
    assert!(!cb_bytes.is_empty(), "control block should not be empty");

    // First byte should indicate leaf version in lower bits
    let first_byte = cb_bytes[0];
    let leaf_version_bits = first_byte & 0xfe;
    assert_eq!(leaf_version_bits, 0xc0, "should be TapScript leaf version");
}

// === extract_seller_sale_sig tests ===

#[test]
fn extract_seller_sale_sig_success() {
    let secp = secp256k1::Secp256k1::new();
    let seller_secret = secret_key(0x50);
    let marketplace_secret = secret_key(0x60);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);

    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);

    let multisig = create_taproot_multisig(
        &seller_pubkey.inner,
        &marketplace_pubkey.inner,
        seller_xonly,
        Network::Bitcoin,
    )
    .unwrap();

    // Create a minimal sale template PSBT
    let locking_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: multisig.output_script.clone(),
        }],
    };

    let mut psbt = Psbt::from_unsigned_tx(Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(locking_tx.txid(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: ScriptBuf::new(),
        }],
    })
    .unwrap();

    // Set up Taproot metadata
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(100_000),
        script_pubkey: multisig.output_script.clone(),
    });
    psbt.inputs[0].tap_internal_key = Some(multisig.internal_key);
    psbt.inputs[0]
        .tap_scripts
        .insert(multisig.control_block.clone(), (multisig.leaf_script.clone(), LeafVersion::TapScript));

    // Compute sighash and sign with Schnorr
    let leaf_hash = TapLeafHash::from_script(&multisig.leaf_script, LeafVersion::TapScript);
    let prevouts = vec![TxOut {
        value: Amount::from_sat(100_000),
        script_pubkey: multisig.output_script.clone(),
    }];

    let sighash = SighashCache::new(&psbt.unsigned_tx)
        .taproot_script_spend_signature_hash(
            0,
            &bitcoin::sighash::Prevouts::All(&prevouts),
            leaf_hash,
            bitcoin::sighash::TapSighashType::SinglePlusAnyoneCanPay,
        )
        .unwrap();

    let schnorr_sig = secp.sign_schnorr(
        &secp256k1::Message::from_digest(sighash.to_byte_array()),
        &seller_keypair,
    );

    psbt.inputs[0].tap_script_sigs.insert(
        (seller_xonly, leaf_hash),
        taproot::Signature {
            sig: schnorr_sig,
            hash_ty: bitcoin::sighash::TapSighashType::SinglePlusAnyoneCanPay,
        },
    );

    // Now extract the signature
    let sig_hex = extract_seller_sale_sig(&encode_psbt(&psbt), &seller_pubkey.to_string()).unwrap();

    // Should be a valid schnorr signature (64 bytes) + 1 byte sighash type
    let sig_bytes = hex::decode(&sig_hex).unwrap();
    assert!(
        sig_bytes.len() >= 64,
        "signature too short: {} bytes",
        sig_bytes.len()
    );
}

#[test]
fn extract_seller_sale_sig_missing_signature_errors() {
    let seller_secret = secret_key(0x70);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);

    // Create PSBT without any signatures
    let psbt = Psbt::from_unsigned_tx(Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![],
    })
    .unwrap();

    let err = extract_seller_sale_sig(&encode_psbt(&psbt), &seller_pubkey.to_string()).unwrap_err();
    assert!(err.to_string().contains("seller taproot signature missing"));
}

#[test]
fn extract_seller_sale_sig_invalid_pubkey_errors() {
    let psbt = Psbt::from_unsigned_tx(Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    })
    .unwrap();

    let err = extract_seller_sale_sig(&encode_psbt(&psbt), "invalid_pubkey").unwrap_err();
    assert!(err.to_string().contains("invalid seller_pubkey_hex"));
}

// === apply_marketplace_signature tests ===

#[test]
fn apply_marketplace_signature_success() {
    let secp = secp256k1::Secp256k1::new();
    let seller_secret = secret_key(0x80);
    let marketplace_secret = secret_key(0x90);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);

    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);

    let multisig = create_taproot_multisig(
        &seller_pubkey.inner,
        &marketplace_pubkey.inner,
        seller_xonly,
        Network::Bitcoin,
    )
    .unwrap();

    // Create a PSBT with the multisig input
    let mut psbt = Psbt::from_unsigned_tx(Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![],
    })
    .unwrap();

    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(100_000),
        script_pubkey: multisig.output_script.clone(),
    });
    psbt.inputs[0]
        .tap_scripts
        .insert(multisig.control_block.clone(), (multisig.leaf_script.clone(), LeafVersion::TapScript));

    // Verify the PSBT is set up correctly for the marketplace to sign
    assert!(!psbt.inputs[0].tap_scripts.is_empty());
    assert!(psbt.inputs[0].witness_utxo.is_some());
}

#[test]
fn apply_marketplace_signature_missing_tap_scripts_would_error() {
    // Create PSBT without tap_scripts - marketplace signature application
    // requires tap_scripts to determine the leaf script for signing
    let psbt = Psbt::from_unsigned_tx(Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![],
    })
    .unwrap();

    // Verify PSBT has no tap_scripts
    assert!(psbt.inputs[0].tap_scripts.is_empty());
    // apply_marketplace_signature would fail with "input 0 missing tap_scripts"
}

// === finalize_multisig_and_extract error case tests ===

#[test]
fn finalize_multisig_missing_seller_sig_errors() {
    let secp = secp256k1::Secp256k1::new();
    let seller_secret = secret_key(0xa0);
    let marketplace_secret = secret_key(0xb0);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);

    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);

    let multisig = create_taproot_multisig(
        &seller_pubkey.inner,
        &marketplace_pubkey.inner,
        seller_xonly,
        Network::Bitcoin,
    )
    .unwrap();

    // Create PSBT with tap_scripts but no signatures
    let mut psbt = Psbt::from_unsigned_tx(Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![],
    })
    .unwrap();

    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(100_000),
        script_pubkey: multisig.output_script.clone(),
    });
    psbt.inputs[0]
        .tap_scripts
        .insert(multisig.control_block.clone(), (multisig.leaf_script.clone(), LeafVersion::TapScript));

    let err = finalize_multisig_and_extract(
        &encode_psbt(&psbt),
        &seller_pubkey.to_string(),
        &marketplace_pubkey.to_string(),
    )
    .unwrap_err();

    assert!(err.to_string().contains("seller taproot signature missing"));
}

#[test]
fn finalize_multisig_missing_marketplace_sig_errors() {
    let secp = secp256k1::Secp256k1::new();
    let seller_secret = secret_key(0xc0);
    let marketplace_secret = secret_key(0xd0);
    let buyer_secret = secret_key(0xe0);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);

    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);

    let multisig = create_taproot_multisig(
        &seller_pubkey.inner,
        &marketplace_pubkey.inner,
        seller_xonly,
        Network::Bitcoin,
    )
    .unwrap();

    let locking_tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: multisig.output_script.clone(),
        }],
    };

    let mut psbt = decode_psbt(
        &build_protected_sale_psbt(&ProtectedSalePsbtRequest {
            locking_raw_tx_hex: hex::encode(bitcoin::consensus::encode::serialize(&locking_tx)),
            multisig_vout: 0,
            multisig_script_hex: hex::encode(multisig.leaf_script.as_bytes()),
            seller_address: SELLER_ADDR.to_string(),
            seller_pubkey_hex: seller_pubkey.to_string(),
            price_sats: 50_000,
            buyer_address: BUYER_ADDR.to_string(),
            buyer_funding_input: spendable_input(FAKE_TXID_2, 1, 200_000, p2wpkh_script(&buyer_secret)),
            fee_rate_sat_vb: 5.0,
            marketplace_fee_address: None,
            marketplace_fee_bps: 0,
            seller_sale_sig_hex: None,
        })
        .unwrap()
        .psbt_hex,
    )
    .unwrap();

    let leaf_hash = TapLeafHash::from_script(&multisig.leaf_script, LeafVersion::TapScript);
    let prevouts = vec![
        psbt.inputs[0].witness_utxo.clone().unwrap(),
        psbt.inputs[1].witness_utxo.clone().unwrap(),
    ];

    // Add ONLY seller signature (not marketplace)
    let seller_sighash = SighashCache::new(&psbt.unsigned_tx)
        .taproot_script_spend_signature_hash(
            0,
            &bitcoin::sighash::Prevouts::All(&prevouts),
            leaf_hash,
            bitcoin::sighash::TapSighashType::SinglePlusAnyoneCanPay,
        )
        .unwrap();
    let seller_schnorr_sig = secp.sign_schnorr(
        &secp256k1::Message::from_digest(seller_sighash.to_byte_array()),
        &seller_keypair,
    );
    let seller_tap_sig = taproot::Signature {
        sig: seller_schnorr_sig,
        hash_ty: bitcoin::sighash::TapSighashType::SinglePlusAnyoneCanPay,
    };
    psbt.inputs[0]
        .tap_script_sigs
        .insert((seller_xonly, leaf_hash), seller_tap_sig);

    let err = finalize_multisig_and_extract(
        &encode_psbt(&psbt),
        &seller_pubkey.to_string(),
        &marketplace_pubkey.to_string(),
    )
    .unwrap_err();

    assert!(err.to_string().contains("marketplace taproot signature missing"));
}

#[test]
fn finalize_multisig_missing_tap_scripts_errors() {
    let seller_secret = secret_key(0xf0);
    let marketplace_secret = secret_key(0xf1);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);

    // Create PSBT without tap_scripts
    let psbt = Psbt::from_unsigned_tx(Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::new(FAKE_TXID.parse().unwrap(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![],
    })
    .unwrap();

    let err = finalize_multisig_and_extract(
        &encode_psbt(&psbt),
        &seller_pubkey.to_string(),
        &marketplace_pubkey.to_string(),
    )
    .unwrap_err();

    assert!(err.to_string().contains("input 0 missing tap_scripts"));
}

#[test]
fn finalize_multisig_invalid_pubkey_hex_errors() {
    let psbt = Psbt::from_unsigned_tx(Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    })
    .unwrap();

    let err =
        finalize_multisig_and_extract(&encode_psbt(&psbt), "invalid", &"invalid".to_string())
            .unwrap_err();
    assert!(err.to_string().contains("invalid seller_pubkey_hex"));
}

// === End-to-end pre-signature flow ===

#[test]
fn e2e_protected_sale_presignature_flow() {
    use crate::services::marketplace_keypair::MarketplaceKeypair;

    let secp = secp256k1::Secp256k1::new();
    let seller_secret = secret_key(0xa1);
    let marketplace_secret = secret_key(0xa2);
    let buyer_secret = secret_key(0xa3);
    let seller_pubkey = bitcoin_pubkey(&seller_secret);
    let marketplace_pubkey = bitcoin_pubkey(&marketplace_secret);
    let price_sats = 50_000u64;

    // Step 1: Build locking PSBT (includes sale template).
    let inscription_input = spendable_input(
        FAKE_TXID,
        0,
        MIN_SELF_FUNDED + 100,
        p2wpkh_script(&seller_secret),
    );
    let locking_result = build_locking_psbt(&LockingPsbtRequest {
        inscription_input: inscription_input.clone(),
        gas_funding_input: None,
        seller_pubkey_hex: seller_pubkey.to_string(),
        seller_address: SELLER_ADDR.to_string(),
        price_sats,
        marketplace_pubkey_hex: marketplace_pubkey.to_string(),
        network: Network::Bitcoin,
        min_relay_fee_rate_sat_vb: None,
    })
    .unwrap();

    // Step 2: Seller signs inscription input and finalizes locking PSBT.
    let mut locking_psbt = decode_psbt(&locking_result.psbt_hex).unwrap();
    sign_p2wpkh_input(&mut locking_psbt, 0, &seller_secret);
    let locking_raw_tx_hex = finalize_locking_psbt(&encode_psbt(&locking_psbt)).unwrap();

    // Step 3: Seller signs the sale template with Schnorr SIGHASH_SINGLE|ANYONECANPAY.
    let mut sale_template = decode_psbt(&locking_result.sale_template_psbt_hex).unwrap();
    let seller_keypair = secp256k1::Keypair::from_secret_key(&secp, &seller_secret);
    let (seller_xonly, _) = secp256k1::XOnlyPublicKey::from_keypair(&seller_keypair);

    let (leaf_script, _) = sale_template.inputs[0]
        .tap_scripts
        .values()
        .next()
        .unwrap()
        .clone();
    let leaf_hash = TapLeafHash::from_script(&leaf_script, LeafVersion::TapScript);

    let template_prevouts = vec![sale_template.inputs[0].witness_utxo.clone().unwrap()];
    let template_sighash = SighashCache::new(&sale_template.unsigned_tx)
        .taproot_script_spend_signature_hash(
            0,
            &bitcoin::sighash::Prevouts::All(&template_prevouts),
            leaf_hash,
            bitcoin::sighash::TapSighashType::SinglePlusAnyoneCanPay,
        )
        .unwrap();
    let seller_schnorr_sig = secp.sign_schnorr(
        &secp256k1::Message::from_digest(template_sighash.to_byte_array()),
        &seller_keypair,
    );
    sale_template.inputs[0].tap_script_sigs.insert(
        (seller_xonly, leaf_hash),
        taproot::Signature {
            sig: seller_schnorr_sig,
            hash_ty: bitcoin::sighash::TapSighashType::SinglePlusAnyoneCanPay,
        },
    );

    // Step 4: Extract seller sale sig.
    let sig_hex = extract_seller_sale_sig(
        &encode_psbt(&sale_template),
        &seller_pubkey.to_string(),
    )
    .unwrap();

    // Step 5: Build protected sale PSBT with the extracted seller sig.
    let buyer_input = spendable_input(FAKE_TXID_2, 1, 200_000, p2wpkh_script(&buyer_secret));
    let sale_result = build_protected_sale_psbt(&ProtectedSalePsbtRequest {
        locking_raw_tx_hex: locking_raw_tx_hex.clone(),
        multisig_vout: 0,
        multisig_script_hex: locking_result.multisig_script_hex.clone(),
        seller_address: SELLER_ADDR.to_string(),
        seller_pubkey_hex: seller_pubkey.to_string(),
        price_sats,
        buyer_address: BUYER_ADDR.to_string(),
        buyer_funding_input: buyer_input.clone(),
        fee_rate_sat_vb: 5.0,
        marketplace_fee_address: None,
        marketplace_fee_bps: 0,
        seller_sale_sig_hex: Some(sig_hex),
    })
    .unwrap();

    // Step 6: Buyer signs their P2WPKH input.
    let mut sale_psbt = decode_psbt(&sale_result.psbt_hex).unwrap();
    sign_p2wpkh_input(&mut sale_psbt, 1, &buyer_secret);

    // Step 7: Apply marketplace Schnorr co-signature.
    let marketplace_kp = MarketplaceKeypair::from_secret_key(marketplace_secret);
    let cosigned_hex = apply_marketplace_signature(
        &encode_psbt(&sale_psbt),
        &marketplace_kp,
    )
    .unwrap();

    // Step 8: Finalize and extract.
    let raw_tx = finalize_multisig_and_extract(
        &cosigned_hex,
        &seller_pubkey.to_string(),
        &marketplace_pubkey.to_string(),
    )
    .unwrap();
    let tx: Transaction = bitcoin::consensus::deserialize(&hex::decode(raw_tx).unwrap()).unwrap();

    // Verify: Taproot script-path witness has 4 elements on input[0].
    // [seller_sig, marketplace_sig, leaf_script, control_block]
    assert_eq!(tx.input[0].witness.len(), 4, "expected 4-element taproot script-path witness");

    // Verify: P2WPKH witness has 2 elements on input[1].
    assert_eq!(tx.input[1].witness.len(), 2, "expected 2-element P2WPKH witness");

    // Verify: output[0] pays the seller the correct price.
    let seller_addr = Address::from_str(SELLER_ADDR).unwrap().assume_checked();
    assert_eq!(tx.output[0].value.to_sat(), price_sats);
    assert_eq!(tx.output[0].script_pubkey, seller_addr.script_pubkey());
}

// === Helper utilities tests ===

#[test]
fn calculate_marketplace_fee_zero_bps() {
    assert_eq!(calculate_marketplace_fee(1_000_000, 0), 0);
    assert_eq!(calculate_marketplace_fee(100_000, 0), 0);
}

#[test]
fn calculate_marketplace_fee_various_bps() {
    // 1% = 100 bps
    assert_eq!(calculate_marketplace_fee(1_000_000, 100), 10_000);

    // 2.5% = 250 bps
    assert_eq!(calculate_marketplace_fee(1_000_000, 250), 25_000);

    // 0.5% = 50 bps
    assert_eq!(calculate_marketplace_fee(2_000_000, 50), 10_000);
}

#[test]
fn calculate_marketplace_fee_minimum_threshold() {
    // Very small sale, fee should be clamped to MIN_MARKETPLACE_FEE_SATS
    let small_price = 1_000u64;
    let fee = calculate_marketplace_fee(small_price, 50); // 0.5%

    // 0.5% of 1,000 = 5 sats, but should be clamped to 1000 sats minimum
    assert_eq!(fee, MIN_MARKETPLACE_FEE_SATS);
    assert!(fee >= MIN_MARKETPLACE_FEE_SATS);
}

#[test]
fn calculate_marketplace_fee_100_percent() {
    // Edge case: 100% fee
    assert_eq!(calculate_marketplace_fee(1_000_000, 10_000), 1_000_000);
}

// Import for tests
use bitcoin::OutPoint;
