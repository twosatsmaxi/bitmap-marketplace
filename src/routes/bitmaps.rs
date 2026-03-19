use crate::{
    errors::{AppError, AppResult},
    AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::Serialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/:block_height/details", get(get_bitmap_details))
}

#[derive(Serialize)]
struct BitmapDetailsResponse {
    block_height: i64,
    inscription_id: String,
    owner: String,
    traits: Vec<String>,
    children_count: usize,
    children: Vec<String>,
    genesis_height: i64,
}

async fn get_bitmap_details(
    State(state): State<AppState>,
    Path(block_height): Path<i64>,
) -> AppResult<Json<BitmapDetailsResponse>> {
    // Query DB: get bitmap by block_height
    let bitmap = state
        .db
        .get_bitmap_by_height(block_height)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound(format!("Bitmap not found for block {}", block_height)))?;

    // Get inscription_id from bitmap
    let inscription_id = bitmap
        .inscription_id
        .ok_or_else(|| AppError::NotFound(format!("No inscription for block {}", block_height)))?;

    // Call OrdClient to get inscription details
    let ord_inscription = state.ord_client
        .get_inscription(&inscription_id)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    // Owner from address field (may be None)
    let owner = ord_inscription
        .address
        .ok_or_else(|| AppError::NotFound(format!("No owner address for inscription {}", inscription_id)))?;

    // Genesis height from ord response (use height field, fallback to genesis_height, then 0)
    let genesis_height = ord_inscription
        .height
        .or(ord_inscription.genesis_height)
        .map(|h| h as i64)
        .unwrap_or(0);

    // Children count and IDs from ord response
    let children_count = ord_inscription.child_count as usize;
    let children = ord_inscription.children;

    Ok(Json(BitmapDetailsResponse {
        block_height,
        inscription_id,
        owner,
        traits: bitmap.traits,
        children_count,
        children,
        genesis_height,
    }))
}

#[cfg(test)]
mod tests {
    use super::BitmapDetailsResponse;

    #[test]
    fn response_serializes_correctly() {
        let resp = BitmapDetailsResponse {
            block_height: 800000,
            inscription_id: "abc123i0".to_string(),
            owner: "bc1pxxx".to_string(),
            traits: vec!["pristine_punk".to_string(), "perfect_punk".to_string()],
            children_count: 5,
            children: vec!["child1i0".to_string(), "child2i0".to_string()],
            genesis_height: 800000,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["block_height"], 800000);
        assert_eq!(json["inscription_id"], "abc123i0");
        assert_eq!(json["owner"], "bc1pxxx");
        assert_eq!(json["children_count"], 5);
        assert_eq!(json["genesis_height"], 800000);
    }
}
