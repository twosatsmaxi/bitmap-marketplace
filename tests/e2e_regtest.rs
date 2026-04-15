// End-to-end integration tests for the bitmap-marketplace buy/sell/list flows
// using regtest bitcoind + testcontainers Postgres.
//
// These tests are #[ignore]d by default. Run with:
//   cargo test --test e2e_regtest -- --ignored --test-threads=1
//
// Requires Docker to be running.

mod regtest_helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use serial_test::serial;
use tower::ServiceExt;

use regtest_helpers::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn json_response(router: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value =
        serde_json::from_slice(&body_bytes).unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&body_bytes).to_string()}));
    (status, body)
}

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Test: Health endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
#[serial]
async fn test_health_returns_200() {
    let infra = TestInfra::start().await;
    let state = build_test_state(&infra).await;
    let router = build_test_router(state);

    let (status, _) = json_response(&router, get("/health")).await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Test: Unprotected listing flow (create listing → buy → confirm)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
#[serial]
async fn test_unprotected_buy_flow() {
    let infra = TestInfra::start().await;
    let state = build_test_state(&infra).await;
    let router = build_test_router(state.clone());
    let rpc = RpcHelper::new(&infra.rpc_url);

    // --- Seller setup ---
    // Create a UTXO that simulates an "inscription" the seller owns.
    let seller_addr = rpc.new_address();
    let inscription_txid = rpc.fund_address(&seller_addr, 0.001); // 100,000 sats
    // Find the vout that pays to the seller
    let inscription_tx = rpc.get_raw_transaction(&inscription_txid);
    let inscription_vout = inscription_tx
        .output
        .iter()
        .position(|o| o.script_pubkey == seller_addr.script_pubkey())
        .expect("seller output not found") as u32;

    let price_sats: i64 = 50_000;

    // Build a listing PSBT (seller signs input 0 with SIGHASH_SINGLE|ANYONECANPAY)
    let listing_psbt = bitmap_marketplace::services::psbt::build_listing_psbt(
        &bitmap_marketplace::services::psbt::ListingRequest {
            inscription_txid: inscription_txid.to_string(),
            inscription_vout,
            seller_address: seller_addr.to_string(),
            price_sats: price_sats as u64,
        },
    )
    .expect("build_listing_psbt failed");

    // Sign the listing PSBT with the seller's wallet
    let listing_psbt_b64 = psbt_hex_to_base64(&listing_psbt.psbt_hex);
    let signed_listing_b64 = rpc.wallet_sign_psbt(&listing_psbt_b64);
    let signed_listing_hex = psbt_base64_to_hex(&signed_listing_b64);

    // --- Create listing via API ---
    let create_body = json!({
        "inscription_id": format!("{}i{}", inscription_txid, inscription_vout),
        "price_sats": price_sats,
        "seller_address": seller_addr.to_string(),
        "unsigned_psbt": signed_listing_hex,
    });
    let (status, body) = json_response(&router, post_json("/api/listings", &create_body)).await;
    assert_eq!(status, StatusCode::OK, "create listing failed: {body}");
    let listing_id = body["id"].as_str().expect("missing listing id");

    // --- Verify listing ---
    let (status, body) =
        json_response(&router, get(&format!("/api/listings/{listing_id}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "active");
    assert_eq!(body["protection_status"], "none");

    // --- Buyer setup ---
    let buyer_addr = rpc.new_address();
    let buyer_funding_txid = rpc.fund_address(&buyer_addr, 0.01); // 1,000,000 sats
    let buyer_tx = rpc.get_raw_transaction(&buyer_funding_txid);
    let buyer_vout = buyer_tx
        .output
        .iter()
        .position(|o| o.script_pubkey == buyer_addr.script_pubkey())
        .expect("buyer output not found") as u32;
    let (buyer_script_hex, buyer_value_sats) =
        rpc.get_utxo_info(&buyer_funding_txid, buyer_vout);

    // --- Build buy PSBT via API ---
    let buy_body = json!({
        "listing_id": listing_id,
        "buyer_address": buyer_addr.to_string(),
        "fee_rate_sat_vb": 1.0,
        "buyer_funding_input": {
            "txid": buyer_funding_txid.to_string(),
            "vout": buyer_vout,
            "value_sats": buyer_value_sats,
            "witness_utxo": {
                "script_pubkey_hex": buyer_script_hex,
                "value_sats": buyer_value_sats,
            },
        },
    });
    let (status, body) = json_response(&router, post_json("/api/orders/buy", &buy_body)).await;
    assert_eq!(status, StatusCode::OK, "buy order failed: {body}");
    let buy_psbt_hex = body["psbt"].as_str().expect("missing psbt in buy response");
    assert_eq!(body["protection_status"], "none");

    // --- Buyer signs the buy PSBT ---
    let buy_psbt_b64 = psbt_hex_to_base64(buy_psbt_hex);
    let signed_buy_b64 = rpc.wallet_sign_psbt(&buy_psbt_b64);
    let signed_buy_hex = psbt_base64_to_hex(&signed_buy_b64);

    // --- Confirm order (finalize + broadcast) ---
    let confirm_body = json!({
        "listing_id": listing_id,
        "signed_psbt": signed_buy_hex,
        "buyer_address": buyer_addr.to_string(),
    });
    let (status, body) =
        json_response(&router, post_json("/api/orders/confirm", &confirm_body)).await;
    assert_eq!(status, StatusCode::OK, "confirm order failed: {body}");
    assert_eq!(body["status"], "broadcast");
    let tx_id = body["tx_id"].as_str().expect("missing tx_id in confirm response");
    assert!(!tx_id.is_empty(), "tx_id should not be empty");

    // --- Verify listing is sold ---
    let (status, body) =
        json_response(&router, get(&format!("/api/listings/{listing_id}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "sold");

    // --- Verify transaction exists on-chain ---
    let txid = bitcoin::Txid::from_raw_hash(
        tx_id.parse::<bitcoin::hashes::sha256d::Hash>().unwrap(),
    );
    let tx = rpc.get_raw_transaction(&txid);
    assert!(!tx.input.is_empty(), "broadcast tx should have inputs");
}

// ---------------------------------------------------------------------------
// Test: Protected listing flow (locking tx + mempool protection)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
#[serial]
async fn test_protected_buy_flow() {
    use bitcoin::secp256k1::{self, Secp256k1, SecretKey};
    use bitcoin::sighash::SighashCache;
    use bitcoin::ecdsa;

    let infra = TestInfra::start().await;
    let state = build_test_state(&infra).await;
    let router = build_test_router(state.clone());
    let rpc = RpcHelper::new(&infra.rpc_url);

    // --- Seller setup ---
    let seller_addr = rpc.new_address();
    let inscription_txid = rpc.fund_address(&seller_addr, 0.001);
    let inscription_tx = rpc.get_raw_transaction(&inscription_txid);
    let inscription_vout = inscription_tx
        .output
        .iter()
        .position(|o| o.script_pubkey == seller_addr.script_pubkey())
        .unwrap() as u32;
    let (inscription_script_hex, inscription_value_sats) =
        rpc.get_utxo_info(&inscription_txid, inscription_vout);

    let price_sats: i64 = 50_000;

    // Seller keypair — we need the secret key for manual signing of the sale template.
    // Use a fixed test key distinct from the marketplace key.
    let secp = Secp256k1::new();
    let seller_secret = SecretKey::from_slice(&[0x02u8; 32]).unwrap();
    let seller_pubkey = bitcoin::PublicKey {
        compressed: true,
        inner: secp256k1::PublicKey::from_secret_key(&secp, &seller_secret),
    };

    // --- Create protected listing via API ---
    let create_body = json!({
        "inscription_id": format!("{}i{}", inscription_txid, inscription_vout),
        "price_sats": price_sats,
        "seller_address": seller_addr.to_string(),
        "seller_pubkey": seller_pubkey.to_string(),
        "inscription_input": {
            "txid": inscription_txid.to_string(),
            "vout": inscription_vout,
            "value_sats": inscription_value_sats,
            "witness_utxo": {
                "script_pubkey_hex": inscription_script_hex,
                "value_sats": inscription_value_sats,
            },
        },
    });
    let (status, body) = json_response(&router, post_json("/api/listings", &create_body)).await;
    assert_eq!(status, StatusCode::OK, "create protected listing failed: {body}");
    let listing_id = body["id"].as_str().expect("missing listing id");
    let locking_psbt_hex = body["locking_psbt"]
        .as_str()
        .expect("missing locking_psbt");
    let sale_template_psbt_hex = body["sale_template_psbt"]
        .as_str()
        .expect("missing sale_template_psbt");

    // Verify listing is in locking_pending state
    let (_, listing_body) =
        json_response(&router, get(&format!("/api/listings/{listing_id}"))).await;
    assert_eq!(listing_body["protection_status"], "locking_pending");

    // --- Seller signs the locking PSBT ---
    // The locking PSBT spends the inscription input → multisig output.
    // The seller's wallet owns the inscription input, so walletprocesspsbt works.
    let locking_b64 = psbt_hex_to_base64(locking_psbt_hex);
    let signed_locking_b64 = rpc.wallet_sign_psbt(&locking_b64);
    let signed_locking_hex = psbt_base64_to_hex(&signed_locking_b64);

    // --- Seller signs the sale template PSBT ---
    // This must be signed manually with the seller's known secret key
    // using SIGHASH_SINGLE|ANYONECANPAY against the multisig input.
    let mut sale_template_psbt =
        bitmap_marketplace::services::psbt::decode_psbt(sale_template_psbt_hex)
            .expect("failed to decode sale template PSBT");

    let witness_script = sale_template_psbt.inputs[0]
        .witness_script
        .clone()
        .expect("sale template missing witness_script");
    let witness_utxo = sale_template_psbt.inputs[0]
        .witness_utxo
        .clone()
        .expect("sale template missing witness_utxo");

    let sighash = SighashCache::new(&sale_template_psbt.unsigned_tx)
        .p2wsh_signature_hash(
            0,
            &witness_script,
            witness_utxo.value,
            bitcoin::EcdsaSighashType::SinglePlusAnyoneCanPay,
        )
        .expect("sighash computation failed");
    let msg = secp256k1::Message::from(sighash);
    let sig = secp.sign_ecdsa(&msg, &seller_secret);
    sale_template_psbt.inputs[0].partial_sigs.insert(
        seller_pubkey,
        ecdsa::Signature {
            sig,
            hash_ty: bitcoin::EcdsaSighashType::SinglePlusAnyoneCanPay,
        },
    );
    let signed_sale_template_hex =
        bitmap_marketplace::services::psbt::encode_psbt(&sale_template_psbt);

    // --- Submit locking ---
    let submit_body = json!({
        "signed_locking_psbt": signed_locking_hex,
        "signed_sale_template_psbt": signed_sale_template_hex,
    });
    let (status, body) = json_response(
        &router,
        post_json(&format!("/api/listings/{listing_id}/submit-locking"), &submit_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "submit-locking failed: {body}");
    assert_eq!(body["protection_status"], "active");

    // --- Verify listing is now active ---
    let (_, listing_body) =
        json_response(&router, get(&format!("/api/listings/{listing_id}"))).await;
    assert_eq!(listing_body["protection_status"], "active");

    // --- Buyer setup ---
    let buyer_addr = rpc.new_address();
    let buyer_funding_txid = rpc.fund_address(&buyer_addr, 0.01);
    let buyer_tx = rpc.get_raw_transaction(&buyer_funding_txid);
    let buyer_vout = buyer_tx
        .output
        .iter()
        .position(|o| o.script_pubkey == buyer_addr.script_pubkey())
        .unwrap() as u32;
    let (buyer_script_hex, buyer_value_sats) =
        rpc.get_utxo_info(&buyer_funding_txid, buyer_vout);

    // --- Build buy PSBT via API ---
    let buy_body = json!({
        "listing_id": listing_id,
        "buyer_address": buyer_addr.to_string(),
        "fee_rate_sat_vb": 1.0,
        "buyer_funding_input": {
            "txid": buyer_funding_txid.to_string(),
            "vout": buyer_vout,
            "value_sats": buyer_value_sats,
            "witness_utxo": {
                "script_pubkey_hex": buyer_script_hex,
                "value_sats": buyer_value_sats,
            },
        },
    });
    let (status, body) = json_response(&router, post_json("/api/orders/buy", &buy_body)).await;
    assert_eq!(status, StatusCode::OK, "protected buy failed: {body}");
    let buy_psbt_hex = body["psbt"].as_str().expect("missing psbt");
    assert_eq!(body["protection_status"], "active");

    // --- Buyer signs their input (input 1) ---
    // The buyer's wallet owns the funding UTXO.
    let buy_b64 = psbt_hex_to_base64(buy_psbt_hex);
    let signed_buy_b64 = rpc.wallet_sign_psbt(&buy_b64);
    let signed_buy_hex = psbt_base64_to_hex(&signed_buy_b64);

    // --- Confirm order (marketplace co-signs + broadcasts package) ---
    let confirm_body = json!({
        "listing_id": listing_id,
        "signed_psbt": signed_buy_hex,
        "buyer_address": buyer_addr.to_string(),
    });
    let (status, body) =
        json_response(&router, post_json("/api/orders/confirm", &confirm_body)).await;
    assert_eq!(status, StatusCode::OK, "confirm protected order failed: {body}");
    assert_eq!(body["status"], "broadcast");

    let sale_tx_id = body["sale_tx_id"]
        .as_str()
        .expect("missing sale_tx_id");
    let locking_tx_id = body["locking_tx_id"]
        .as_str()
        .expect("missing locking_tx_id");
    assert!(!sale_tx_id.is_empty());
    assert!(!locking_tx_id.is_empty());

    // --- Verify listing is sold ---
    let (_, listing_body) =
        json_response(&router, get(&format!("/api/listings/{listing_id}"))).await;
    assert_eq!(listing_body["status"], "sold");

    // --- Mine a block and verify transactions are confirmed ---
    rpc.mine_blocks(1);
    let sale_txid = bitcoin::Txid::from_raw_hash(
        sale_tx_id.parse::<bitcoin::hashes::sha256d::Hash>().unwrap(),
    );
    let sale_tx = rpc.get_raw_transaction(&sale_txid);
    assert!(!sale_tx.input.is_empty());
}

// ---------------------------------------------------------------------------
// Test: Listing cancellation
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
#[serial]
async fn test_cancel_listing() {
    let infra = TestInfra::start().await;
    let state = build_test_state(&infra).await;
    let router = build_test_router(state);

    // Create a simple listing
    let create_body = json!({
        "inscription_id": "0000000000000000000000000000000000000000000000000000000000000001i0",
        "price_sats": 100_000,
        "seller_address": "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080",
    });
    let (status, body) = json_response(&router, post_json("/api/listings", &create_body)).await;
    assert_eq!(status, StatusCode::OK, "create listing failed: {body}");
    let listing_id = body["id"].as_str().unwrap();

    // Cancel
    let (status, body) =
        json_response(&router, delete(&format!("/api/listings/{listing_id}"))).await;
    assert_eq!(status, StatusCode::OK, "cancel failed: {body}");
    assert_eq!(body["status"], "cancelled");

    // Verify
    let (_, body) = json_response(&router, get(&format!("/api/listings/{listing_id}"))).await;
    assert_eq!(body["status"], "cancelled");
}

// ---------------------------------------------------------------------------
// Test: Buy non-existent listing → 404
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
#[serial]
async fn test_buy_nonexistent_listing() {
    let infra = TestInfra::start().await;
    let state = build_test_state(&infra).await;
    let router = build_test_router(state);

    let buy_body = json!({
        "listing_id": "00000000-0000-0000-0000-000000000000",
        "buyer_address": "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080",
        "fee_rate_sat_vb": 1.0,
        "buyer_funding_input": {
            "txid": "0000000000000000000000000000000000000000000000000000000000000001",
            "vout": 0,
            "value_sats": 100000,
            "witness_utxo": {
                "script_pubkey_hex": "0014751e76e8199196d454941c45d1b3a323f1433bd6",
                "value_sats": 100000,
            },
        },
    });
    let (status, _) = json_response(&router, post_json("/api/orders/buy", &buy_body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Test: Buy listing in locking_pending state → 409
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
#[serial]
async fn test_buy_locking_pending_listing() {
    let infra = TestInfra::start().await;
    let state = build_test_state(&infra).await;
    let router = build_test_router(state.clone());
    let rpc = RpcHelper::new(&infra.rpc_url);

    // Create a protected listing (stays in locking_pending until submit-locking)
    let seller_addr = rpc.new_address();
    let inscription_txid = rpc.fund_address(&seller_addr, 0.001);
    let inscription_tx = rpc.get_raw_transaction(&inscription_txid);
    let inscription_vout = inscription_tx
        .output
        .iter()
        .position(|o| o.script_pubkey == seller_addr.script_pubkey())
        .unwrap() as u32;
    let (script_hex, value_sats) = rpc.get_utxo_info(&inscription_txid, inscription_vout);

    let secp = bitcoin::secp256k1::Secp256k1::new();
    let seller_secret = bitcoin::secp256k1::SecretKey::from_slice(&[0x03u8; 32]).unwrap();
    let seller_pubkey = bitcoin::PublicKey {
        compressed: true,
        inner: bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &seller_secret),
    };

    let create_body = json!({
        "inscription_id": format!("{}i{}", inscription_txid, inscription_vout),
        "price_sats": 50_000,
        "seller_address": seller_addr.to_string(),
        "seller_pubkey": seller_pubkey.to_string(),
        "inscription_input": {
            "txid": inscription_txid.to_string(),
            "vout": inscription_vout,
            "value_sats": value_sats,
            "witness_utxo": {
                "script_pubkey_hex": script_hex,
                "value_sats": value_sats,
            },
        },
    });
    let (status, body) = json_response(&router, post_json("/api/listings", &create_body)).await;
    assert_eq!(status, StatusCode::OK, "create listing failed: {body}");
    let listing_id = body["id"].as_str().unwrap();

    // Try to buy while still in locking_pending → should get 409
    let buy_body = json!({
        "listing_id": listing_id,
        "buyer_address": "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080",
        "fee_rate_sat_vb": 1.0,
        "buyer_funding_input": {
            "txid": "0000000000000000000000000000000000000000000000000000000000000099",
            "vout": 0,
            "value_sats": 1000000,
            "witness_utxo": {
                "script_pubkey_hex": "0014751e76e8199196d454941c45d1b3a323f1433bd6",
                "value_sats": 1000000,
            },
        },
    });
    let (status, body) = json_response(&router, post_json("/api/orders/buy", &buy_body)).await;
    assert_eq!(status, StatusCode::CONFLICT, "expected 409 for locking_pending listing: {body}");
}
