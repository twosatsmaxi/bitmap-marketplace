use crate::{
    errors::{AppError, AppResult},
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:block_height/details", get(get_bitmap_details))
        .route("/:block_height/children", get(get_bitmap_children))
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

#[derive(Deserialize)]
struct ChildrenQuery {
    #[serde(default)]
    page: u64,
    #[serde(default = "default_children_limit")]
    limit: u64,
}

fn default_children_limit() -> u64 {
    20
}

#[derive(Serialize)]
struct BitmapChildrenResponse {
    block_height: i64,
    parent_inscription_id: String,
    total_children: u64,
    page: u64,
    children: Vec<crate::services::ord::OrdInscription>,
    has_more: bool,
}

async fn get_bitmap_children(
    State(state): State<AppState>,
    Path(block_height): Path<i64>,
    Query(query): Query<ChildrenQuery>,
) -> AppResult<Json<BitmapChildrenResponse>> {
    let bitmap = state
        .db
        .get_bitmap_by_height(block_height)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound(format!("Bitmap not found for block {}", block_height)))?;

    let inscription_id = bitmap
        .inscription_id
        .ok_or_else(|| AppError::NotFound(format!("No inscription for block {}", block_height)))?;

    let ord_inscription = state
        .ord_client
        .get_inscription(&inscription_id)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let total_children = ord_inscription.child_count;
    let limit = query.limit.clamp(1, 50);
    let offset = query.page * limit;

    let indices: Vec<u64> = (offset..total_children.min(offset + limit)).collect();
    let has_more = offset + limit < total_children;

    let futs: Vec<_> = indices
        .into_iter()
        .map(|child_index| {
            let client = state.ord_client.clone();
            let id = inscription_id.clone();
            async move {
                client.get_child_inscription(&id, child_index).await
            }
        })
        .collect();

    let children: Vec<crate::services::ord::OrdInscription> = stream::iter(futs)
        .buffer_unordered(10)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::Internal)?;

    Ok(Json(BitmapChildrenResponse {
        block_height,
        parent_inscription_id: inscription_id,
        total_children,
        page: query.page,
        children,
        has_more,
    }))
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
