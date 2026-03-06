use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use crate::{
    errors::{AppError, AppResult},
    models::offer::{Offer, OfferStatus},
    models::activity::{Activity, ActivityType},
    ws::WsEvent,
    AppState,
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
    /// Optional PSBT the buyer has pre-signed at their offered price
    psbt: Option<String>,
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
        psbt: req.psbt,
        expires_at: Some(expires_at),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    let created = state.db.create_offer(&offer).await.map_err(AppError::Internal)?;

    // Activity: log offer received
    let activity = Activity {
        id: Uuid::new_v4(),
        inscription_id: req.inscription_id.clone(),
        collection_id: None,
        activity_type: ActivityType::List, // closest available; offer-specific type TBD
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

    if offer.status != OfferStatus::Pending {
        return Err(AppError::BadRequest(format!(
            "Offer is not pending (status: {:?})",
            offer.status
        )));
    }

    // Check expiry
    if let Some(exp) = offer.expires_at {
        if Utc::now() > exp {
            let _ = state
                .db
                .update_offer_status(id, OfferStatus::Expired)
                .await;
            return Err(AppError::BadRequest("Offer has expired".into()));
        }
    }

    state
        .db
        .update_offer_status(id, OfferStatus::Accepted)
        .await
        .map_err(AppError::Internal)?;

    // Activity
    let activity = Activity {
        id: Uuid::new_v4(),
        inscription_id: offer.inscription_id.clone(),
        collection_id: None,
        activity_type: ActivityType::Sale,
        from_address: None,
        to_address: Some(offer.buyer_address.clone()),
        price_sats: Some(offer.price_sats),
        tx_id: None,
        block_height: None,
        created_at: Utc::now(),
    };
    let _ = state.db.create_activity(&activity).await;

    // Broadcast sale confirmed
    state.ws_broadcaster.send(WsEvent::SaleConfirmed {
        inscription_id: offer.inscription_id.clone(),
        price_sats: offer.price_sats as u64,
        buyer: offer.buyer_address.clone(),
        tx_id: "pending_broadcast".to_string(),
    });

    Ok(Json(serde_json::json!({
        "offer_id": id,
        "status": "accepted",
        "inscription_id": offer.inscription_id,
        "buyer_address": offer.buyer_address,
        "price_sats": offer.price_sats,
    })))
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
        return Err(AppError::BadRequest("Only pending offers can be cancelled".into()));
    }

    state
        .db
        .update_offer_status(id, OfferStatus::Cancelled)
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(serde_json::json!({ "offer_id": id, "status": "cancelled" })))
}
