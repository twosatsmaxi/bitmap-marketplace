use crate::{
    db::Database,
    errors::{AppError, AppResult},
    models::activity::{Activity, ActivityType},
    models::listing::ListingStatus,
    models::offer::{Offer, OfferStatus},
    models::sale::Sale,
    services::psbt::{
        apply_marketplace_signature, calculate_marketplace_fee, finalize_and_extract,
        finalize_multisig_and_extract, txid_from_raw_hex,
    },
    ws::WsEvent,
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_offer))
        .route("/", get(list_offers))
        .route("/:id/accept", post(accept_offer))
        .route("/:id", delete(cancel_offer))
}

#[derive(Deserialize)]
struct ListOffersQuery {
    inscription_id: String,
}

#[derive(Deserialize)]
struct CreateOfferRequest {
    inscription_id: String,
    buyer_address: String,
    price_sats: i64,
    /// PSBT the buyer has pre-signed at their offered price
    psbt: String,
    /// TTL in seconds (default 86400 = 24h)
    ttl_seconds: Option<i64>,
}

async fn create_offer(
    State(state): State<AppState>,
    Json(req): Json<CreateOfferRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if req.price_sats <= 0 {
        return Err(AppError::BadRequest("price_sats must be positive".into()));
    }

    let ttl = req.ttl_seconds.unwrap_or(86_400);
    let expires_at = Utc::now() + chrono::Duration::seconds(ttl);

    let offer = Offer {
        id: Uuid::new_v4(),
        inscription_id: req.inscription_id.clone(),
        buyer_address: req.buyer_address.clone(),
        price_sats: req.price_sats,
        status: OfferStatus::Pending,
        psbt: Some(req.psbt),
        expires_at: Some(expires_at),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    let created = state
        .db
        .create_offer(&offer)
        .await
        .map_err(AppError::Internal)?;

    // Activity: log offer received
    let activity = Activity {
        id: Uuid::new_v4(),
        inscription_id: req.inscription_id.clone(),
        collection_id: None,
        activity_type: ActivityType::Offer,
        from_address: Some(req.buyer_address.clone()),
        to_address: None,
        price_sats: Some(req.price_sats),
        tx_id: None,
        block_height: None,
        created_at: Utc::now(),
    };
    let _ = state.db.create_activity(&activity).await;

    // Broadcast WS
    state.ws_broadcaster.send(WsEvent::OfferReceived {
        inscription_id: req.inscription_id,
        price_sats: req.price_sats as u64,
        buyer: req.buyer_address,
    });

    Ok(Json(serde_json::json!(created)))
}

async fn list_offers(
    State(state): State<AppState>,
    Query(q): Query<ListOffersQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let offers = state
        .db
        .get_offers_by_inscription(&q.inscription_id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "offers": offers })))
}

async fn accept_offer(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let offer = state
        .db
        .get_offer(id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("Offer not found".into()))?;

    let signed_psbt = match validate_offer_for_acceptance(&offer, Utc::now())? {
        OfferAcceptanceCheck::Expired => {
            let _ = state.db.update_offer_status(id, OfferStatus::Expired).await;
            return Err(AppError::BadRequest("Offer has expired".into()));
        }
        OfferAcceptanceCheck::Ready(psbt) => psbt,
    };

    // Look up active listing for this inscription
    let listing = state
        .db
        .get_active_listing_by_inscription(&offer.inscription_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("No active listing found for this inscription".into()))?;

    let rpc = crate::services::bitcoin_rpc::BitcoinRpc::new().map_err(AppError::Internal)?;

    let marketplace_fee_sats =
        calculate_marketplace_fee(offer.price_sats as u64, state.marketplace_fee_bps) as i64;

    // Broadcast based on protection status
    let (sale_tx_id, locking_tx_id) = if listing.protection_status == "active" {
        // Protected flow: apply marketplace co-sig, finalize P2WSH, broadcast package
        let locking_raw_tx = listing
            .locking_raw_tx
            .as_ref()
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("missing locking_raw_tx")))?;
        let seller_pubkey = listing
            .seller_pubkey
            .as_ref()
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("missing seller_pubkey")))?;

        let cosigned_psbt = apply_marketplace_signature(signed_psbt, &state.marketplace_keypair)
            .map_err(AppError::Internal)?;

        let sale_raw_tx = finalize_multisig_and_extract(
            &cosigned_psbt,
            seller_pubkey,
            &state.marketplace_keypair.pubkey_hex(),
        )
        .map_err(AppError::Internal)?;

        // Pre-broadcast UTXO liveness check.
        rpc.verify_inputs_unspent(locking_raw_tx)
            .map_err(|e| AppError::Conflict(e.to_string()))?;

        let lock_txid = txid_from_raw_hex(locking_raw_tx).map_err(AppError::Internal)?;
        let sale_txid = txid_from_raw_hex(&sale_raw_tx).map_err(AppError::Internal)?;

        rpc.submit_package(&[locking_raw_tx, &sale_raw_tx])
            .map_err(AppError::Internal)?;

        (sale_txid, Some(lock_txid))
    } else {
        // Unprotected flow: finalize and broadcast
        let raw_tx = finalize_and_extract(signed_psbt).map_err(AppError::Internal)?;
        let tx_id = rpc
            .broadcast_transaction(&raw_tx)
            .map_err(AppError::Internal)?;
        (tx_id, None)
    };

    // All DB writes in a transaction
    let mut tx = state.db.pool.begin().await.map_err(AppError::Database)?;

    // Accept the offer
    sqlx::query("UPDATE offers SET status = $1, updated_at = NOW() WHERE id = $2")
        .bind(OfferStatus::Accepted)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    // Update listing to sold
    sqlx::query("UPDATE listings SET status = $1, updated_at = NOW() WHERE id = $2")
        .bind(ListingStatus::Sold)
        .bind(listing.id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    // Create sale record
    let sale = Sale {
        id: Uuid::new_v4(),
        listing_id: Some(listing.id),
        inscription_id: offer.inscription_id.clone(),
        seller_address: listing.seller_address.clone(),
        buyer_address: offer.buyer_address.clone(),
        price_sats: offer.price_sats,
        marketplace_fee_sats,
        tx_id: Some(sale_tx_id.clone()),
        locking_tx_id: locking_tx_id.clone(),
        block_height: None,
        confirmed_at: None,
        created_at: Utc::now(),
    };
    Database::create_sale_in_tx(&mut *tx, &sale)
        .await
        .map_err(AppError::Internal)?;

    // Update inscription ownership
    sqlx::query(
        "UPDATE inscriptions SET owner_address = $1, updated_at = NOW() WHERE inscription_id = $2",
    )
    .bind(&offer.buyer_address)
    .bind(&offer.inscription_id)
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    // Cancel other pending offers on the same inscription
    sqlx::query(
        "UPDATE offers SET status = 'cancelled', updated_at = NOW() WHERE inscription_id = $1 AND id != $2 AND status = 'pending'",
    )
    .bind(&offer.inscription_id)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    tx.commit().await.map_err(AppError::Database)?;

    // Activity log (best-effort, outside transaction)
    let activity = Activity {
        id: Uuid::new_v4(),
        inscription_id: offer.inscription_id.clone(),
        collection_id: None,
        activity_type: ActivityType::Sale,
        from_address: Some(listing.seller_address.clone()),
        to_address: Some(offer.buyer_address.clone()),
        price_sats: Some(offer.price_sats),
        tx_id: Some(sale_tx_id.clone()),
        block_height: None,
        created_at: Utc::now(),
    };
    let _ = state.db.create_activity(&activity).await;

    // Broadcast WS
    state.ws_broadcaster.send(WsEvent::SaleConfirmed {
        inscription_id: offer.inscription_id.clone(),
        price_sats: offer.price_sats as u64,
        buyer: offer.buyer_address.clone(),
        tx_id: sale_tx_id.clone(),
    });

    Ok(Json(serde_json::json!({
        "offer_id": id,
        "status": "accepted",
        "inscription_id": offer.inscription_id,
        "buyer_address": offer.buyer_address,
        "price_sats": offer.price_sats,
        "tx_id": sale_tx_id,
    })))
}

#[derive(Debug)]
enum OfferAcceptanceCheck<'a> {
    Ready(&'a str),
    Expired,
}

fn validate_offer_for_acceptance(
    offer: &Offer,
    now: chrono::DateTime<Utc>,
) -> AppResult<OfferAcceptanceCheck<'_>> {
    if offer.status != OfferStatus::Pending {
        return Err(AppError::BadRequest(format!(
            "Offer is not pending (status: {:?})",
            offer.status
        )));
    }

    if let Some(exp) = offer.expires_at {
        if now > exp {
            return Ok(OfferAcceptanceCheck::Expired);
        }
    }

    let signed_psbt = offer
        .psbt
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("Offer has no PSBT".into()))?;

    Ok(OfferAcceptanceCheck::Ready(signed_psbt))
}

#[cfg(test)]
mod tests {
    use super::{validate_offer_for_acceptance, OfferAcceptanceCheck};
    use crate::{
        errors::AppError,
        models::offer::{Offer, OfferStatus},
    };
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    fn make_offer(
        status: OfferStatus,
        expires_at: Option<chrono::DateTime<Utc>>,
        psbt: Option<&str>,
    ) -> Offer {
        let now = Utc::now();
        Offer {
            id: Uuid::new_v4(),
            inscription_id: "inscription".to_string(),
            buyer_address: "buyer".to_string(),
            price_sats: 1_000,
            status,
            psbt: psbt.map(str::to_string),
            expires_at,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn validate_offer_acceptance_rejects_non_pending_status() {
        let offer = make_offer(OfferStatus::Accepted, None, Some("psbt"));

        let err = validate_offer_for_acceptance(&offer, Utc::now()).unwrap_err();
        match err {
            AppError::BadRequest(message) => assert!(message.contains("not pending")),
            other => panic!("expected bad request, got {other:?}"),
        }
    }

    #[test]
    fn validate_offer_acceptance_marks_expired_offer() {
        let now = Utc::now();
        let offer = make_offer(
            OfferStatus::Pending,
            Some(now - Duration::seconds(1)),
            Some("psbt"),
        );

        let result = validate_offer_for_acceptance(&offer, now).unwrap();
        assert!(matches!(result, OfferAcceptanceCheck::Expired));
    }

    #[test]
    fn validate_offer_acceptance_requires_psbt() {
        let offer = make_offer(OfferStatus::Pending, None, None);

        let err = validate_offer_for_acceptance(&offer, Utc::now()).unwrap_err();
        match err {
            AppError::BadRequest(message) => assert!(message.contains("no PSBT")),
            other => panic!("expected bad request, got {other:?}"),
        }
    }

    #[test]
    fn validate_offer_acceptance_returns_psbt_for_ready_offer() {
        let now = Utc::now();
        let offer = make_offer(
            OfferStatus::Pending,
            Some(now + Duration::seconds(30)),
            Some("signed-psbt"),
        );

        let result = validate_offer_for_acceptance(&offer, now).unwrap();
        match result {
            OfferAcceptanceCheck::Ready(psbt) => assert_eq!(psbt, "signed-psbt"),
            OfferAcceptanceCheck::Expired => panic!("expected ready offer"),
        }
    }

    #[test]
    fn validate_offer_acceptance_allows_offer_expiring_exactly_now() {
        let now = Utc::now();
        let offer = make_offer(OfferStatus::Pending, Some(now), Some("signed-psbt"));

        let result = validate_offer_for_acceptance(&offer, now).unwrap();
        match result {
            OfferAcceptanceCheck::Ready(psbt) => assert_eq!(psbt, "signed-psbt"),
            OfferAcceptanceCheck::Expired => panic!("offer should not expire exactly at now"),
        }
    }

    #[test]
    fn validate_offer_acceptance_allows_offer_without_expiry() {
        let offer = make_offer(OfferStatus::Pending, None, Some("signed-psbt"));

        let result = validate_offer_for_acceptance(&offer, Utc::now()).unwrap();
        match result {
            OfferAcceptanceCheck::Ready(psbt) => assert_eq!(psbt, "signed-psbt"),
            OfferAcceptanceCheck::Expired => panic!("offer without expiry should stay valid"),
        }
    }
}

async fn cancel_offer(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let offer = state
        .db
        .get_offer(id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("Offer not found".into()))?;

    if offer.status != OfferStatus::Pending {
        return Err(AppError::BadRequest(
            "Only pending offers can be cancelled".into(),
        ));
    }

    state
        .db
        .update_offer_status(id, OfferStatus::Cancelled)
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(
        serde_json::json!({ "offer_id": id, "status": "cancelled" }),
    ))
}
