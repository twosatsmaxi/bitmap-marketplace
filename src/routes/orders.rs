use crate::{
    db::Database,
    errors::{AppError, AppResult},
    models::activity::{Activity, ActivityType},
    models::listing::BuyListingRequest,
    models::sale::Sale,
    services::psbt::{
        apply_marketplace_signature, build_buy_psbt, build_protected_sale_psbt,
        calculate_marketplace_fee, finalize_and_extract, finalize_multisig_and_extract,
        txid_from_raw_hex, BuyRequest, ProtectedSalePsbtRequest, SpendableInput,
    },
    ws::WsEvent,
    AppState,
};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/buy", post(buy))
        .route("/confirm", post(confirm_order))
        .route("/:id", get(get_order))
}

/// POST /orders/buy
/// Returns a PSBT for the buyer to sign.
/// - Protected listings (protection_status = 'active'): returns a protected sale PSBT.
/// - Unprotected listings (protection_status = 'none'): returns the legacy buy PSBT.
async fn buy(
    State(state): State<AppState>,
    Json(req): Json<BuyListingRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let listing = state
        .db
        .get_listing(req.listing_id)
        .await
        .map_err(AppError::Internal)?;
    let listing = listing.ok_or_else(|| AppError::NotFound("Listing not found".to_string()))?;

    if listing.protection_status == "locking_pending" {
        return Err(AppError::Conflict(
            "Listing is waiting for seller to submit the locking transaction. Try again shortly."
                .to_string(),
        ));
    }

    if listing.protection_status == "active" {
        // Protected flow: spend from locking tx.
        let locking_raw_tx = listing.locking_raw_tx.ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!(
                "listing is active but missing locking_raw_tx"
            ))
        })?;
        let multisig_script = listing.multisig_script.ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!(
                "listing is active but missing multisig_script"
            ))
        })?;
        let seller_pubkey = listing.seller_pubkey.ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!(
                "listing is active but missing seller_pubkey"
            ))
        })?;

        let buyer_funding_input = req.buyer_funding_input.as_ref().ok_or_else(|| {
            AppError::BadRequest("buyer_funding_input required for protected purchase".to_string())
        })?;

        let psbt_req = ProtectedSalePsbtRequest {
            locking_raw_tx_hex: locking_raw_tx,
            multisig_vout: 0,
            multisig_script_hex: multisig_script,
            seller_address: listing.seller_address.clone(),
            seller_pubkey_hex: seller_pubkey,
            price_sats: listing.price_sats as u64,
            buyer_address: req.buyer_address.clone(),
            buyer_funding_input: SpendableInput::from(buyer_funding_input),
            fee_rate_sat_vb: req.fee_rate_sat_vb.unwrap_or(5.0),
            marketplace_fee_address: state.marketplace_fee_address.clone(),
            marketplace_fee_bps: state.marketplace_fee_bps,
            seller_sale_sig_hex: listing.seller_sale_sig,
        };

        let result = build_protected_sale_psbt(&psbt_req).map_err(AppError::Internal)?;

        return Ok(Json(serde_json::json!({
            "psbt": result.psbt_hex,
            "estimated_fee_sats": result.estimated_fee_sats,
            "marketplace_fee_sats": result.marketplace_fee_sats,
            "locking_txid": result.locking_txid,
            "protection_status": "active",
        })));
    }

    // Legacy flow: unprotected listing.
    let seller_psbt_hex = listing
        .psbt
        .ok_or_else(|| AppError::BadRequest("listing has no PSBT".to_string()))?;

    let buy_req = BuyRequest {
        seller_psbt_hex,
        buyer_address: req.buyer_address.clone(),
        buyer_funding_input: SpendableInput::from(
            req.buyer_funding_input
                .as_ref()
                .ok_or_else(|| AppError::BadRequest("buyer_funding_input required".to_string()))?,
        ),
        fee_rate_sat_vb: req.fee_rate_sat_vb.unwrap_or(5.0),
        marketplace_fee_address: state.marketplace_fee_address.clone(),
        marketplace_fee_bps: state.marketplace_fee_bps,
    };

    let result = build_buy_psbt(&buy_req).map_err(AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "psbt": result.psbt_hex,
        "estimated_fee_sats": result.estimated_fee_sats,
        "marketplace_fee_sats": result.marketplace_fee_sats,
        "protection_status": "none",
    })))
}

#[derive(Deserialize)]
struct ConfirmOrderRequest {
    listing_id: Uuid,
    signed_psbt: String,
    /// buyer_address is needed to populate the Sale row.
    buyer_address: Option<String>,
}

/// POST /orders/confirm
/// For unprotected: finalize the signed PSBT and broadcast via sendrawtransaction.
/// For protected: apply marketplace co-sig, finalize P2WSH witness, broadcast via submitpackage.
async fn confirm_order(
    State(state): State<AppState>,
    Json(req): Json<ConfirmOrderRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let listing = state
        .db
        .get_listing(req.listing_id)
        .await
        .map_err(AppError::Internal)?;
    let listing = listing.ok_or_else(|| AppError::NotFound("Listing not found".to_string()))?;

    let rpc = crate::services::bitcoin_rpc::BitcoinRpc::new().map_err(AppError::Internal)?;

    let buyer_address = req.buyer_address.unwrap_or_else(|| "unknown".to_string());

    let marketplace_fee_sats =
        calculate_marketplace_fee(listing.price_sats as u64, state.marketplace_fee_bps) as i64;

    if listing.protection_status == "active" {
        // Protected flow.
        let locking_raw_tx = listing
            .locking_raw_tx
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("missing locking_raw_tx")))?;
        let seller_pubkey = listing
            .seller_pubkey
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("missing seller_pubkey")))?;

        let cosigned_psbt =
            apply_marketplace_signature(&req.signed_psbt, &state.marketplace_keypair)
                .map_err(AppError::Internal)?;

        let sale_raw_tx = finalize_multisig_and_extract(
            &cosigned_psbt,
            &seller_pubkey,
            &state.marketplace_keypair.pubkey_hex(),
        )
        .map_err(AppError::Internal)?;

        // Pre-broadcast UTXO liveness check: verify locking tx inputs are still unspent.
        rpc.verify_inputs_unspent(&locking_raw_tx)
            .map_err(|e| AppError::Conflict(e.to_string()))?;

        // Compute txids from raw hex before broadcast so we don't depend on
        // HashMap iteration order from submitpackage's tx-results.
        let locking_txid = txid_from_raw_hex(&locking_raw_tx).map_err(AppError::Internal)?;
        let sale_txid = txid_from_raw_hex(&sale_raw_tx).map_err(AppError::Internal)?;

        rpc.submit_package(&[&locking_raw_tx, &sale_raw_tx])
            .map_err(AppError::Internal)?;

        // Wrap all DB writes in a transaction for atomicity.
        let mut tx = state.db.pool.begin().await.map_err(AppError::Database)?;

        sqlx::query("UPDATE listings SET status = $1, updated_at = NOW() WHERE id = $2")
            .bind(crate::models::listing::ListingStatus::Sold)
            .bind(listing.id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;

        let sale = Sale {
            id: Uuid::new_v4(),
            listing_id: Some(listing.id),
            inscription_id: listing.inscription_id.clone(),
            seller_address: listing.seller_address.clone(),
            buyer_address: buyer_address.clone(),
            price_sats: listing.price_sats,
            marketplace_fee_sats,
            tx_id: Some(sale_txid.clone()),
            locking_tx_id: Some(locking_txid.clone()),
            block_height: None,
            confirmed_at: None,
            created_at: chrono::Utc::now(),
        };
        Database::create_sale_in_tx(&mut *tx, &sale)
            .await
            .map_err(AppError::Internal)?;

        sqlx::query(
            "UPDATE inscriptions SET owner_address = $1, updated_at = NOW() WHERE inscription_id = $2",
        )
        .bind(&buyer_address)
        .bind(&listing.inscription_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

        tx.commit().await.map_err(AppError::Database)?;

        state.ws_broadcaster.send(WsEvent::SaleConfirmed {
            inscription_id: listing.inscription_id.clone(),
            price_sats: listing.price_sats as u64,
            buyer: buyer_address,
            tx_id: sale_txid.clone(),
        });

        return Ok(Json(serde_json::json!({
            "listing_id": req.listing_id,
            "status": "broadcast",
            "locking_tx_id": locking_txid,
            "sale_tx_id": sale_txid,
        })));
    }

    // Legacy flow: finalize and broadcast via sendrawtransaction.
    let raw_tx = finalize_and_extract(&req.signed_psbt).map_err(AppError::Internal)?;

    let tx_id = rpc
        .broadcast_transaction(&raw_tx)
        .map_err(|e| AppError::Internal(e))?;

    // Wrap all DB writes in a transaction for atomicity.
    let mut tx = state.db.pool.begin().await.map_err(AppError::Database)?;

    sqlx::query("UPDATE listings SET status = $1, updated_at = NOW() WHERE id = $2")
        .bind(crate::models::listing::ListingStatus::Sold)
        .bind(listing.id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    let sale = Sale {
        id: Uuid::new_v4(),
        listing_id: Some(listing.id),
        inscription_id: listing.inscription_id.clone(),
        seller_address: listing.seller_address.clone(),
        buyer_address: buyer_address.clone(),
        price_sats: listing.price_sats,
        marketplace_fee_sats,
        tx_id: Some(tx_id.clone()),
        locking_tx_id: None,
        block_height: None,
        confirmed_at: None,
        created_at: chrono::Utc::now(),
    };
    Database::create_sale_in_tx(&mut *tx, &sale)
        .await
        .map_err(AppError::Internal)?;

    sqlx::query(
        "UPDATE inscriptions SET owner_address = $1, updated_at = NOW() WHERE inscription_id = $2",
    )
    .bind(&buyer_address)
    .bind(&listing.inscription_id)
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    tx.commit().await.map_err(AppError::Database)?;

    // Record activity at confirmation time (not at PSBT build time).
    let activity = Activity {
        id: Uuid::new_v4(),
        inscription_id: listing.inscription_id.clone(),
        collection_id: None,
        activity_type: ActivityType::Sale,
        from_address: Some(listing.seller_address.clone()),
        to_address: Some(buyer_address.clone()),
        price_sats: Some(listing.price_sats),
        tx_id: Some(tx_id.clone()),
        block_height: None,
        created_at: chrono::Utc::now(),
    };
    let _ = state.db.create_activity(&activity).await;

    state.ws_broadcaster.send(WsEvent::SaleConfirmed {
        inscription_id: listing.inscription_id.clone(),
        price_sats: listing.price_sats as u64,
        buyer: buyer_address,
        tx_id: tx_id.clone(),
    });

    Ok(Json(serde_json::json!({
        "listing_id": req.listing_id,
        "status": "broadcast",
        "tx_id": tx_id,
    })))
}

async fn get_order(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let listing = state.db.get_listing(id).await.map_err(AppError::Internal)?;
    match listing {
        Some(l) => Ok(Json(serde_json::json!(l))),
        None => Err(AppError::NotFound("Order not found".to_string())),
    }
}
