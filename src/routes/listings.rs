use crate::{
    db::listings::{ListingFilter, ListingSort},
    errors::{AppError, AppResult},
    models::activity::{Activity, ActivityType},
    models::listing::{CreateListingRequest, Listing, ListingStatus, SpendableInputRequest},
    services::magic_eden,
    services::psbt::{
        build_locking_psbt, decode_psbt, extract_seller_sale_sig, LockingPsbtRequest,
        SpendableInput, WitnessUtxo,
    },
    ws::WsEvent,
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use bitcoin::secp256k1::PublicKey;
use serde::{Deserialize, Deserializer};
use std::str::FromStr;
use uuid::Uuid;

fn map_spendable_input(input: &SpendableInputRequest) -> SpendableInput {
    SpendableInput {
        txid: input.txid.clone(),
        vout: input.vout,
        value_sats: input.value_sats,
        witness_utxo: WitnessUtxo {
            script_pubkey_hex: input.witness_utxo.script_pubkey_hex.clone(),
            value_sats: input.witness_utxo.value_sats,
        },
        non_witness_utxo_hex: input.non_witness_utxo_hex.clone(),
        redeem_script_hex: input.redeem_script_hex.clone(),
        witness_script_hex: input.witness_script_hex.clone(),
        sequence: input.sequence,
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_listings).post(create_listing))
        .route("/import", post(import_listings))
        .route("/:id", get(get_listing).delete(cancel_listing))
        .route("/:id/confirm", post(confirm_listing))
        .route("/:id/submit-locking", post(submit_locking))
}

#[derive(Debug, Deserialize)]
struct Pagination {
    limit: Option<i64>,
    offset: Option<i64>,
    collection_id: Option<Uuid>,
    seller_address: Option<String>,
    min_price_sats: Option<i64>,
    max_price_sats: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_listing_sort")]
    sort_by: Option<ListingSort>,
}

async fn list_listings(
    State(state): State<AppState>,
    Query(pagination): Query<Pagination>,
) -> AppResult<Json<serde_json::Value>> {
    let limit = pagination.limit.unwrap_or(50);
    let offset = pagination.offset.unwrap_or(0);
    let filter = ListingFilter {
        collection_id: pagination.collection_id,
        seller_address: pagination.seller_address,
        min_price_sats: pagination.min_price_sats,
        max_price_sats: pagination.max_price_sats,
        sort_by: pagination.sort_by,
    };
    let listings = state
        .db
        .list_active_listings(limit, offset, &filter)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "listings": listings })))
}

fn deserialize_listing_sort<'de, D>(deserializer: D) -> Result<Option<ListingSort>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    match value.as_deref() {
        None => Ok(None),
        Some("created_at") => Ok(Some(ListingSort::CreatedAt)),
        Some("price_asc") => Ok(Some(ListingSort::PriceAsc)),
        Some("price_desc") => Ok(Some(ListingSort::PriceDesc)),
        Some(other) => Err(serde::de::Error::custom(format!(
            "invalid sort_by '{other}', expected one of: created_at, price_asc, price_desc"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::Pagination;
    use crate::db::listings::ListingSort;

    #[test]
    fn pagination_deserializes_supported_sort_values() {
        let created_at: Pagination = serde_json::from_value(serde_json::json!({
            "sort_by": "created_at"
        }))
        .expect("created_at should parse");
        let price_asc: Pagination = serde_json::from_value(serde_json::json!({
            "sort_by": "price_asc"
        }))
        .expect("price_asc should parse");
        let price_desc: Pagination = serde_json::from_value(serde_json::json!({
            "sort_by": "price_desc"
        }))
        .expect("price_desc should parse");

        assert!(matches!(created_at.sort_by, Some(ListingSort::CreatedAt)));
        assert!(matches!(price_asc.sort_by, Some(ListingSort::PriceAsc)));
        assert!(matches!(price_desc.sort_by, Some(ListingSort::PriceDesc)));
    }

    #[test]
    fn pagination_rejects_invalid_sort_value() {
        let err = serde_json::from_value::<Pagination>(serde_json::json!({
            "sort_by": "rarity"
        }))
        .expect_err("invalid sort should fail");
        assert!(err.to_string().contains("invalid sort_by"));
    }

    #[test]
    fn pagination_defaults_filters_to_none() {
        let pagination: Pagination =
            serde_json::from_value(serde_json::json!({})).expect("empty query should parse");

        assert!(pagination.collection_id.is_none());
        assert!(pagination.seller_address.is_none());
        assert!(pagination.min_price_sats.is_none());
        assert!(pagination.max_price_sats.is_none());
        assert!(pagination.sort_by.is_none());
        assert!(pagination.limit.is_none());
        assert!(pagination.offset.is_none());
    }
}

async fn create_listing(
    State(state): State<AppState>,
    Json(req): Json<CreateListingRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // If seller_pubkey is provided, build the locking PSBT for mempool protection.
    let (locking_psbt_hex, sale_template_psbt_hex, multisig_address, multisig_script, protection_status) =
        if let Some(ref seller_pubkey_hex) = req.seller_pubkey {
            // Validate the pubkey parses.
            PublicKey::from_str(seller_pubkey_hex).map_err(|_| {
                AppError::BadRequest(
                    "seller_pubkey is not a valid secp256k1 compressed public key".to_string(),
                )
            })?;

            let inscription_input = req.inscription_input.as_ref().ok_or_else(|| {
                AppError::BadRequest("inscription_input required for protected listing".to_string())
            })?;

            let locking_req = LockingPsbtRequest {
                inscription_input: map_spendable_input(inscription_input),
                gas_funding_input: req.gas_funding_input.as_ref().map(map_spendable_input),
                seller_pubkey_hex: seller_pubkey_hex.clone(),
                marketplace_pubkey_hex: state.marketplace_keypair.pubkey_hex(),
                network: state.network,
                min_relay_fee_rate_sat_vb: None,
                seller_address: req.seller_address.clone(),
                price_sats: req.price_sats as u64,
            };

            let locking = build_locking_psbt(&locking_req).map_err(|e| AppError::Internal(e))?;

            (
                Some(locking.psbt_hex),
                Some(locking.sale_template_psbt_hex),
                Some(locking.multisig_address),
                Some(locking.multisig_script_hex),
                "locking_pending".to_string(),
            )
        } else {
            (None, None, None, None, "none".to_string())
        };

    let listing = Listing {
        id: Uuid::new_v4(),
        inscription_id: req.inscription_id.clone(),
        seller_address: req.seller_address.clone(),
        price_sats: req.price_sats,
        status: ListingStatus::Active,
        psbt: req.unsigned_psbt.clone(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        seller_pubkey: req.seller_pubkey.clone(),
        multisig_address,
        multisig_script,
        locking_raw_tx: None, // set after seller signs and calls /submit-locking
        seller_sale_sig: None, // set after seller signs the sale template
        protection_status,
        source_marketplace: None,
    };

    let created = state
        .db
        .create_listing(&listing)
        .await
        .map_err(AppError::Internal)?;

    // Insert Activity
    let activity = Activity {
        id: Uuid::new_v4(),
        inscription_id: req.inscription_id.clone(),
        collection_id: None,
        activity_type: ActivityType::List,
        from_address: Some(req.seller_address.clone()),
        to_address: None,
        price_sats: Some(req.price_sats),
        tx_id: None,
        block_height: None,
        created_at: chrono::Utc::now(),
    };
    let _ = state.db.create_activity(&activity).await;

    // Broadcast WS event
    state.ws_broadcaster.send(WsEvent::NewListing {
        inscription_id: req.inscription_id,
        price_sats: req.price_sats as u64,
        seller: req.seller_address,
    });

    let mut resp = serde_json::json!(created);
    if let Some(psbt_hex) = locking_psbt_hex {
        resp["locking_psbt"] = serde_json::Value::String(psbt_hex);
    }
    if let Some(sale_template_hex) = sale_template_psbt_hex {
        resp["sale_template_psbt"] = serde_json::Value::String(sale_template_hex);
    }

    Ok(Json(resp))
}

async fn get_listing(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let listing = state.db.get_listing(id).await.map_err(AppError::Internal)?;
    match listing {
        Some(l) => Ok(Json(serde_json::json!(l))),
        None => Err(AppError::NotFound("Listing not found".to_string())),
    }
}

async fn cancel_listing(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let listing = state.db.get_listing(id).await.map_err(AppError::Internal)?;
    let listing = listing.ok_or_else(|| AppError::NotFound("Listing not found".to_string()))?;

    // If locking tx is stored but not yet broadcast (protection_status = 'locking_pending' or 'active'),
    // we can simply null it out — the seller never broadcast it so no on-chain cleanup needed.
    if listing.locking_raw_tx.is_some() {
        state
            .db
            .clear_locking_tx(id)
            .await
            .map_err(AppError::Internal)?;
    }

    state
        .db
        .update_listing_status(id, ListingStatus::Cancelled)
        .await
        .map_err(AppError::Internal)?;

    // Insert Activity
    let activity = Activity {
        id: Uuid::new_v4(),
        inscription_id: listing.inscription_id.clone(),
        collection_id: None,
        activity_type: ActivityType::Delist,
        from_address: Some(listing.seller_address.clone()),
        to_address: None,
        price_sats: None,
        tx_id: None,
        block_height: None,
        created_at: chrono::Utc::now(),
    };
    let _ = state.db.create_activity(&activity).await;

    Ok(Json(serde_json::json!({ "status": "cancelled" })))
}

#[derive(Deserialize)]
struct ConfirmListingRequest {
    signed_psbt: String,
}

async fn confirm_listing(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ConfirmListingRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // Ensure the listing exists
    let listing = state.db.get_listing(id).await.map_err(AppError::Internal)?;
    let _ = listing.ok_or_else(|| AppError::NotFound("Listing not found".to_string()))?;

    // Store the buyer-signed PSBT on the listing row
    state
        .db
        .update_listing_psbt(id, &req.signed_psbt)
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "listing_id": id,
        "status": "pending_broadcast"
    })))
}

#[derive(Deserialize)]
struct SubmitLockingRequest {
    /// Hex of the seller-signed locking PSBT (or raw transaction hex).
    signed_locking_psbt: String,
    /// Hex of the seller-signed sale template PSBT (signed with SIGHASH_SINGLE|ANYONECANPAY).
    signed_sale_template_psbt: String,
}

/// POST /:id/submit-locking
/// Seller calls this after signing BOTH the locking PSBT and the sale template PSBT.
/// Validates and finalizes the locking PSBT, extracts the seller's sale pre-signature,
/// stores both, and marks the listing active.
/// Nothing is broadcast here — the locking tx is held until purchase time.
async fn submit_locking(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SubmitLockingRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let listing = state.db.get_listing(id).await.map_err(AppError::Internal)?;
    let listing = listing.ok_or_else(|| AppError::NotFound("Listing not found".to_string()))?;

    // Only accept for listings in locking_pending state.
    if listing.protection_status != "locking_pending" {
        return Err(AppError::Conflict(format!(
            "listing {} is not in locking_pending state (current: {})",
            id, listing.protection_status
        )));
    }

    let seller_pubkey = listing.seller_pubkey.as_deref().ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!("listing in locking_pending but missing seller_pubkey"))
    })?;

    // Validate it's a valid PSBT hex (structural check).
    decode_psbt(&req.signed_locking_psbt)
        .map_err(|e| AppError::BadRequest(format!("invalid locking PSBT: {}", e)))?;

    // Extract the raw transaction from the signed PSBT.
    let raw_tx_hex = crate::services::psbt::finalize_locking_psbt(&req.signed_locking_psbt)
        .map_err(|e| AppError::BadRequest(format!("could not finalize locking PSBT: {}", e)))?;

    // Extract seller's partial_sig from the signed sale template.
    let seller_sale_sig = extract_seller_sale_sig(&req.signed_sale_template_psbt, seller_pubkey)
        .map_err(|e| {
            AppError::BadRequest(format!("could not extract seller sale signature: {}", e))
        })?;

    // Store locking tx, seller sale sig, and activate.
    state
        .db
        .update_locking_tx(id, &raw_tx_hex, Some(&seller_sale_sig), "active")
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "listing_id": id,
        "protection_status": "active",
        "message": "Locking transaction and seller sale signature stored. Listing is now protected and purchasable."
    })))
}

#[derive(Deserialize)]
struct ImportListingsRequest {
    seller_address: String,
}

/// POST /import
/// Fetches active listings from Magic Eden for a given seller address and mirrors them
/// in our marketplace using the already-signed PSBTs — no re-signing required.
async fn import_listings(
    State(state): State<AppState>,
    Json(req): Json<ImportListingsRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let me_listings = magic_eden::fetch_listings_by_seller(&state.http_client, &req.seller_address)
        .await
        .map_err(AppError::Internal)?;

    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut imported_listings = Vec::new();
    let now = chrono::Utc::now();

    for me in me_listings {
        // Skip if already active on our marketplace
        match state
            .db
            .get_active_listing_by_inscription(&me.inscription_id)
            .await
        {
            Ok(Some(_)) => {
                skipped += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!("DB error checking inscription {}: {}", me.inscription_id, e);
                failed += 1;
                continue;
            }
            Ok(None) => {}
        }

        let listing = Listing {
            id: Uuid::new_v4(),
            inscription_id: me.inscription_id.clone(),
            seller_address: me.seller_address.clone(),
            price_sats: me.price_sats,
            status: ListingStatus::Active,
            psbt: me.signed_psbt,
            created_at: now,
            updated_at: now,
            seller_pubkey: None,
            multisig_address: None,
            multisig_script: None,
            locking_raw_tx: None,
            seller_sale_sig: None,
            protection_status: "none".to_string(),
            source_marketplace: Some(magic_eden::NAME.to_string()),
        };

        match state.db.create_listing(&listing).await {
            Ok(created) => {
                state.ws_broadcaster.send(WsEvent::NewListing {
                    inscription_id: me.inscription_id,
                    price_sats: me.price_sats as u64,
                    seller: me.seller_address,
                });
                imported_listings.push(serde_json::json!(created));
                imported += 1;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to insert imported listing {}: {}",
                    me.inscription_id,
                    e
                );
                failed += 1;
            }
        }
    }

    Ok(Json(serde_json::json!({
        "imported": imported,
        "skipped": skipped,
        "failed": failed,
        "listings": imported_listings,
    })))
}
