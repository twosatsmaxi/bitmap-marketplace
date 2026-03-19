use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{errors::AppError, AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/:address", get(get_portfolio))
}

#[derive(Debug, Deserialize)]
pub struct PortfolioQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_limit")]
    pub limit: u64,
}

fn default_limit() -> u64 {
    24
}

#[derive(Debug, Serialize)]
pub struct PortfolioBitmapItem {
    pub block_height: i64,
    pub inscription_id: Option<String>,
    pub inscription_num: Option<i64>,
    pub tx_count: Option<i32>,
    pub block_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    pub traits: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PortfolioResponse {
    pub address: String,
    pub bitmaps: Vec<PortfolioBitmapItem>,
    pub total: i64,
    pub page: u64,
    pub has_more: bool,
}

async fn get_portfolio(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(query): Query<PortfolioQuery>,
) -> Result<Json<PortfolioResponse>, AppError> {
    let limit = query.limit.clamp(1, 50) as i64;
    let page = query.page;
    let offset = (page * query.limit) as i64;

    // Fetch all inscription IDs owned by this address from Ord
    let inscription_ids = state
        .ord_client
        .get_address_inscription_ids(&address)
        .await
        .map_err(AppError::Internal)?;

    if inscription_ids.is_empty() {
        return Ok(Json(PortfolioResponse {
            address,
            bitmaps: vec![],
            total: 0,
            page,
            has_more: false,
        }));
    }

    // Count how many of those inscriptions are bitmaps
    let total = state
        .db
        .count_bitmaps_by_inscription_ids(&inscription_ids)
        .await
        .map_err(AppError::Internal)?;

    // Fetch the paginated subset
    let bitmaps = state
        .db
        .get_bitmaps_by_inscription_ids(&inscription_ids, limit, offset)
        .await
        .map_err(AppError::Internal)?;

    let has_more = (offset + bitmaps.len() as i64) < total;

    let items: Vec<PortfolioBitmapItem> = bitmaps
        .into_iter()
        .map(|b| PortfolioBitmapItem {
            block_height: b.block_height,
            inscription_id: b.inscription_id,
            inscription_num: b.inscription_num,
            tx_count: b.tx_count,
            block_timestamp: b.block_timestamp,
            traits: b.traits,
        })
        .collect();

    Ok(Json(PortfolioResponse {
        address,
        bitmaps: items,
        total,
        page,
        has_more,
    }))
}
