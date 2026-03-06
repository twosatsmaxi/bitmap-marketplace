use axum::{
    extract::{Path, State, Query},
    routing::{get, post},
    Json, Router,
};
use crate::{
    errors::{AppError, AppResult}, 
    AppState,
    models::listing::{Listing, ListingStatus, CreateListingRequest},
    models::activity::{Activity, ActivityType},
    ws::WsEvent,
};
use serde::Deserialize;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_listings).post(create_listing))
        .route("/:id", get(get_listing).delete(cancel_listing))
        .route("/:id/confirm", post(confirm_listing))
}

#[derive(Deserialize)]
struct Pagination {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_listings(
    State(state): State<AppState>,
    Query(pagination): Query<Pagination>,
) -> AppResult<Json<serde_json::Value>> {
    let limit = pagination.limit.unwrap_or(50);
    let offset = pagination.offset.unwrap_or(0);
    let listings = state.db.list_active_listings(limit, offset).await.map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "listings": listings })))
}

async fn create_listing(
    State(state): State<AppState>,
    Json(req): Json<CreateListingRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let listing = Listing {
        id: Uuid::new_v4(),
        inscription_id: req.inscription_id.clone(),
        seller_address: req.seller_address.clone(),
        price_sats: req.price_sats,
        status: ListingStatus::Active,
        psbt: Some(req.unsigned_psbt),
        royalty_address: None,
        royalty_bps: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    
    let created = state.db.create_listing(&listing).await.map_err(AppError::Internal)?;
    
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
    
    Ok(Json(serde_json::json!(created)))
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

    state.db.update_listing_status(id, ListingStatus::Cancelled).await.map_err(AppError::Internal)?;

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
    state.db.update_listing_psbt(id, &req.signed_psbt).await.map_err(AppError::Internal)?;

    // PSBT broadcast service not yet complete; mark as pending_broadcast
    Ok(Json(serde_json::json!({
        "listing_id": id,
        "status": "pending_broadcast"
    })))
}