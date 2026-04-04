use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{errors::AppError, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:address", get(get_portfolio))
        .route("/multi", post(get_multi_portfolio))
}

#[derive(Debug, Deserialize)]
pub struct PortfolioQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_limit")]
    pub limit: u64,
    /// Filter by trait (optional)
    pub trait_filter: Option<String>,
}

fn default_limit() -> u64 {
    24
}

#[derive(Debug, Serialize)]
pub struct TraitStat {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct PortfolioBitmapItem {
    pub block_height: i64,
    pub inscription_id: Option<String>,
    pub inscription_num: Option<i64>,
    pub tx_count: Option<i32>,
    pub block_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    pub traits: Vec<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PortfolioResponse {
    pub address: String,
    pub bitmaps: Vec<PortfolioBitmapItem>,
    pub traits: Vec<TraitStat>,
    pub total: i64,
    pub page: u64,
    pub has_more: bool,
}

#[derive(Debug, Deserialize)]
pub struct MultiPortfolioRequest {
    pub addresses: Vec<String>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_limit")]
    pub limit: u64,
    pub trait_filter: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MultiPortfolioResponse {
    pub addresses: Vec<String>,
    pub bitmaps: Vec<PortfolioBitmapItem>,
    pub traits: Vec<TraitStat>,
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
    let offset = page as i64 * limit;

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
            traits: vec![],
            total: 0,
            page,
            has_more: false,
        }));
    }

    // Get trait counts for all user's bitmaps
    let trait_stats: Vec<TraitStat> = state
        .db
        .get_trait_counts_by_inscription_ids(&inscription_ids)
        .await
        .map_err(AppError::Internal)?
        .into_iter()
        .map(|(name, count)| TraitStat { name, count })
        .collect();

    // Apply trait filter if specified
    let (bitmaps, total) = if let Some(ref trait_filter) = query.trait_filter {
        // Get bitmaps matching the trait
        let filtered_bitmaps = state
            .db
            .get_bitmaps_by_inscription_ids_and_trait(&inscription_ids, trait_filter, limit, offset)
            .await
            .map_err(AppError::Internal)?;
        
        let filtered_total = state
            .db
            .count_bitmaps_by_inscription_ids_and_trait(&inscription_ids, trait_filter)
            .await
            .map_err(AppError::Internal)?;
        
        (filtered_bitmaps, filtered_total)
    } else {
        // No filter - get all bitmaps
        let total = state
            .db
            .count_bitmaps_by_inscription_ids(&inscription_ids)
            .await
            .map_err(AppError::Internal)?;
        
        let bitmaps = state
            .db
            .get_bitmaps_by_inscription_ids(&inscription_ids, limit, offset)
            .await
            .map_err(AppError::Internal)?;
        
        (bitmaps, total)
    };

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
            owner: Some(address.clone()),
        })
        .collect();

    Ok(Json(PortfolioResponse {
        address,
        bitmaps: items,
        traits: trait_stats,
        total,
        page,
        has_more,
    }))
}

async fn get_multi_portfolio(
    State(state): State<AppState>,
    Json(req): Json<MultiPortfolioRequest>,
) -> Result<Json<MultiPortfolioResponse>, AppError> {
    if req.addresses.is_empty() || req.addresses.len() > 100 {
        return Err(AppError::BadRequest(
            "addresses must contain 1-100 items".into(),
        ));
    }

    let limit = req.limit.clamp(1, 50) as i64;
    let page = req.page;
    let offset = page as i64 * limit;

    // Fetch inscription IDs for all addresses in parallel
    let futures: Vec<_> = req
        .addresses
        .iter()
        .map(|addr| {
            let client = state.ord_client.clone();
            let addr = addr.clone();
            async move {
                let ids = client.get_address_inscription_ids(&addr).await?;
                Ok::<_, anyhow::Error>((addr, ids))
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    // Build inscription_id -> owner mapping and merged list
    let mut inscription_to_owner: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut all_inscription_ids: Vec<String> = Vec::new();

    for result in results {
        let (addr, ids) = result.map_err(AppError::Internal)?;
        for id in ids {
            if !inscription_to_owner.contains_key(&id) {
                inscription_to_owner.insert(id.clone(), addr.clone());
                all_inscription_ids.push(id);
            }
        }
    }

    const MAX_INSCRIPTION_IDS: usize = 50_000;
    if all_inscription_ids.len() > MAX_INSCRIPTION_IDS {
        all_inscription_ids.truncate(MAX_INSCRIPTION_IDS);
    }

    if all_inscription_ids.is_empty() {
        return Ok(Json(MultiPortfolioResponse {
            addresses: req.addresses,
            bitmaps: vec![],
            traits: vec![],
            total: 0,
            page,
            has_more: false,
        }));
    }

    // Get trait counts for all bitmaps across all addresses
    let trait_stats: Vec<TraitStat> = state
        .db
        .get_trait_counts_by_inscription_ids(&all_inscription_ids)
        .await
        .map_err(AppError::Internal)?
        .into_iter()
        .map(|(name, count)| TraitStat { name, count })
        .collect();

    // Get bitmaps with optional trait filter
    let (bitmaps, total) = if let Some(ref trait_filter) = req.trait_filter {
        let filtered = state
            .db
            .get_bitmaps_by_inscription_ids_and_trait(
                &all_inscription_ids,
                trait_filter,
                limit,
                offset,
            )
            .await
            .map_err(AppError::Internal)?;
        let count = state
            .db
            .count_bitmaps_by_inscription_ids_and_trait(&all_inscription_ids, trait_filter)
            .await
            .map_err(AppError::Internal)?;
        (filtered, count)
    } else {
        let total = state
            .db
            .count_bitmaps_by_inscription_ids(&all_inscription_ids)
            .await
            .map_err(AppError::Internal)?;
        let bitmaps = state
            .db
            .get_bitmaps_by_inscription_ids(&all_inscription_ids, limit, offset)
            .await
            .map_err(AppError::Internal)?;
        (bitmaps, total)
    };

    let has_more = (offset + bitmaps.len() as i64) < total;

    let items: Vec<PortfolioBitmapItem> = bitmaps
        .into_iter()
        .map(|b| {
            let owner = b
                .inscription_id
                .as_ref()
                .and_then(|iid| inscription_to_owner.get(iid))
                .cloned();
            PortfolioBitmapItem {
                block_height: b.block_height,
                inscription_id: b.inscription_id,
                inscription_num: b.inscription_num,
                tx_count: b.tx_count,
                block_timestamp: b.block_timestamp,
                traits: b.traits,
                owner,
            }
        })
        .collect();

    Ok(Json(MultiPortfolioResponse {
        addresses: req.addresses,
        bitmaps: items,
        traits: trait_stats,
        total,
        page,
        has_more,
    }))
}
