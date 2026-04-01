use super::{
    build_buy_psbt, build_listing_psbt, build_locking_psbt, build_multisig_redeem_script,
    build_protected_sale_psbt, decode_psbt, encode_psbt, finalize_locking_psbt,
    finalize_multisig_and_extract, BuyRequest, ListingRequest, LockingPsbtRequest,
    ProtectedSalePsbtRequest, SpendableInput, WitnessUtxo, MIN_SELF_FUNDED,
};
use bitcoin::{
    ecdsa, key::TapTweak, psbt::Psbt, secp256k1, sighash::SighashCache, taproot, Address, Amount,
    Network, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
};
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
        marketplace_pubkey_hex: bitcoin_pubkey(&secret_key(7)).to_string(),
        network: Network::Bitcoin,
        min_relay_fee_rate_sat_vb: None,
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 50_000,
    };

    let result = build_locking_psbt(&req).unwrap();
    let psbt = decode_psbt(&result.psbt_hex).unwrap();

    assert_eq!(psbt.inputs.len(), 1);
    assert_eq!(
        psbt.inputs[0].witness_utxo.as_ref().unwrap().value.to_sat(),
        inscription_input.value_sats
    );
    assert!(psbt.inputs[0].witness_script.is_none());

    // Verify sale template output[0] uses real seller_address and price_sats
    let sale_template = decode_psbt(&result.sale_template_psbt_hex).unwrap();
    assert_eq!(sale_template.unsigned_tx.output.len(), 1);
    assert_eq!(
        sale_template.unsigned_tx.output[0].value.to_sat(),
        req.price_sats
    );
    let expected_seller_script = Address::from_str(SELLER_ADDR)
        .unwrap()
        .assume_checked()
        .script_pubkey();
    assert_eq!(
        sale_template.unsigned_tx.output[0].script_pubkey,
        expected_seller_script
    );
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
        marketplace_pubkey_hex: bitcoin_pubkey(&secret_key(10)).to_string(),
        network: Network::Bitcoin,
        min_relay_fee_rate_sat_vb: None,
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 50_000,
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
        marketplace_pubkey_hex: bitcoin_pubkey(&secret_key(14)).to_string(),
        network: Network::Bitcoin,
        min_relay_fee_rate_sat_vb: Some(1.0),
        seller_address: SELLER_ADDR.to_string(),
        price_sats: 50_000,
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
    let witness_script =
        build_multisig_redeem_script(&seller_pubkey.inner, &marketplace_pubkey.inner);
    let multisig_address = Address::p2wsh(&witness_script, Network::Bitcoin);
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
            script_pubkey: multisig_address.script_pubkey(),
        }],
    };
    let buyer_input = spendable_input(FAKE_TXID_2, 1, 100_000, p2wpkh_script(&secret_key(17)));

    let result = build_protected_sale_psbt(&ProtectedSalePsbtRequest {
        locking_raw_tx_hex: hex::encode(bitcoin::consensus::encode::serialize(&locking_tx)),
        multisig_vout: 0,
        multisig_script_hex: hex::encode(witness_script.as_bytes()),
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

    assert_eq!(
        psbt.inputs[0].witness_script.as_ref().unwrap(),
        &witness_script
    );
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
    let witness_script =
        build_multisig_redeem_script(&seller_pubkey.inner, &marketplace_pubkey.inner);
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
            script_pubkey: Address::p2wsh(&witness_script, Network::Bitcoin).script_pubkey(),
        }],
    };
    let buyer_input = spendable_input(FAKE_TXID_2, 1, 200_000, p2wpkh_script(&buyer_secret));
    let mut psbt = decode_psbt(
        &build_protected_sale_psbt(&ProtectedSalePsbtRequest {
            locking_raw_tx_hex: hex::encode(bitcoin::consensus::encode::serialize(&locking_tx)),
            multisig_vout: 0,
            multisig_script_hex: hex::encode(witness_script.as_bytes()),
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

    let seller_sighash = SighashCache::new(&psbt.unsigned_tx)
        .p2wsh_signature_hash(
            0,
            &witness_script,
            psbt.inputs[0].witness_utxo.as_ref().unwrap().value,
            bitcoin::EcdsaSighashType::SinglePlusAnyoneCanPay,
        )
        .unwrap();
    let seller_sig = secp.sign_ecdsa(&secp256k1::Message::from(seller_sighash), &seller_secret);
    psbt.inputs[0].partial_sigs.insert(
        seller_pubkey,
        ecdsa::Signature {
            sig: seller_sig,
            hash_ty: bitcoin::EcdsaSighashType::SinglePlusAnyoneCanPay,
        },
    );

    let marketplace_sighash = SighashCache::new(&psbt.unsigned_tx)
        .p2wsh_signature_hash(
            0,
            &witness_script,
            psbt.inputs[0].witness_utxo.as_ref().unwrap().value,
            bitcoin::EcdsaSighashType::All,
        )
        .unwrap();
    let marketplace_sig = secp.sign_ecdsa(
        &secp256k1::Message::from(marketplace_sighash),
        &marketplace_secret,
    );
    psbt.inputs[0].partial_sigs.insert(
        marketplace_pubkey,
        ecdsa::Signature {
            sig: marketplace_sig,
            hash_ty: bitcoin::EcdsaSighashType::All,
        },
    );

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

    assert_eq!(tx.input[0].witness.len(), 4);
    assert_eq!(tx.input[1].witness.len(), 2);
}
