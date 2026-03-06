use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use crate::{
    errors::{AppError, AppResult},
    AppState,
    models::listing::BuyListingRequest,
    models::activity::{Activity, ActivityType},
    ws::WsEvent,
};
use serde::Deserialize;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/buy", post(buy))
        .route("/offer", post(make_offer))
        .route("/confirm", post(confirm_order))
        .route("/:id", get(get_order))
}

async fn buy(
    State(state): State<AppState>,
    Json(req): Json<BuyListingRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let listing = state.db.get_listing(req.listing_id).await.map_err(AppError::Internal)?;
    let listing = listing.ok_or_else(|| AppError::NotFound("Listing not found".to_string()))?;

    state.db.update_listing_status(listing.id, crate::models::listing::ListingStatus::Sold).await.map_err(AppError::Internal)?;

    // Insert Activity
    let activity = Activity {
        id: Uuid::new_v4(),
        inscription_id: listing.inscription_id.clone(),
        collection_id: None,
        activity_type: ActivityType::Sale,
        from_address: Some(listing.seller_address.clone()),
        to_address: Some(req.buyer_address.clone()),
        price_sats: Some(listing.price_sats),
        tx_id: None,
        block_height: None,
        created_at: chrono::Utc::now(),
    };
    let _ = state.db.create_activity(&activity).await;

    // Broadcast WS Event
    state.ws_broadcaster.send(WsEvent::SaleConfirmed {
        inscription_id: listing.inscription_id,
        price_sats: listing.price_sats as u64,
        buyer: req.buyer_address,
        tx_id: "pending".to_string(), // Real implementation would broadcast PSBT and get txid
    });

    Ok(Json(serde_json::json!({ "status": "sold", "listing_id": req.listing_id })))
}

async fn make_offer(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> AppResult<Json<serde_json::Value>> {
    // Currently unimplemented DB logic for offers, but we return OK for now.
    Ok(Json(serde_json::json!({ "offer": body })))
}

#[derive(Deserialize)]
struct ConfirmOrderRequest {
    listing_id: Uuid,
    signed_psbt: String,
}

async fn confirm_order(
    State(state): State<AppState>,
    Json(req): Json<ConfirmOrderRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // Ensure the listing exists
    let listing = state.db.get_listing(req.listing_id).await.map_err(AppError::Internal)?;
    let listing = listing.ok_or_else(|| AppError::NotFound("Listing not found".to_string()))?;

    // Store the signed PSBT on the listing row
    state.db.update_listing_psbt(req.listing_id, &req.signed_psbt).await.map_err(AppError::Internal)?;

    // Emit WS event to notify subscribers of pending broadcast
    state.ws_broadcaster.send(WsEvent::SaleConfirmed {
        inscription_id: listing.inscription_id.clone(),
        price_sats: listing.price_sats as u64,
        buyer: "pending".to_string(),
        tx_id: "pending_broadcast".to_string(),
    });

    Ok(Json(serde_json::json!({
        "listing_id": req.listing_id,
        "status": "pending_broadcast"
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
