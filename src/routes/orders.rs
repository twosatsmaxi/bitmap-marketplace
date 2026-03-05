use axum::{extract::State, routing::post, Json, Router};
use crate::{
    errors::{AppError, AppResult}, 
    AppState,
    models::listing::BuyListingRequest,
    models::activity::{Activity, ActivityType},
    ws::WsEvent,
};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/buy", post(buy))
        .route("/offer", post(make_offer))
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