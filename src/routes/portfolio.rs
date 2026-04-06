use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::get,
    Json, Router,
};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{errors::AppError, routes::auth, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mine", get(get_my_portfolio))
        .route("/profile/:profile_id", get(get_portfolio_by_profile_id))
        .route("/:address", get(get_portfolio))
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

#[derive(Debug, Serialize)]
pub struct WalletEntry {
    pub address: String,
    pub label: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProfilePortfolioResponse {
    pub profile_id: String,
    pub addresses: Vec<WalletEntry>,
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

    // Run trait counts, bitmap fetch, and count concurrently
    let trait_future = state.db.get_trait_counts_by_inscription_ids(&inscription_ids);

    let (bitmaps, total, trait_raw) = if let Some(ref trait_filter) = query.trait_filter {
        tokio::try_join!(
            state.db.get_bitmaps_by_inscription_ids_and_trait(&inscription_ids, trait_filter, limit, offset),
            state.db.count_bitmaps_by_inscription_ids_and_trait(&inscription_ids, trait_filter),
            trait_future,
        )
    } else {
        tokio::try_join!(
            state.db.get_bitmaps_by_inscription_ids(&inscription_ids, limit, offset),
            state.db.count_bitmaps_by_inscription_ids(&inscription_ids),
            trait_future,
        )
    }
    .map_err(AppError::Internal)?;

    let trait_stats: Vec<TraitStat> = trait_raw
        .into_iter()
        .map(|(name, count)| TraitStat { name, count })
        .collect();

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

/// GET /api/portfolio/mine — authenticated; returns aggregated portfolio for all linked wallets
async fn get_my_portfolio(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PortfolioQuery>,
) -> Result<Json<ProfilePortfolioResponse>, AppError> {
    let profile = auth::authenticate(&headers, &state).await?;
    let wallets = state.db.get_profile_wallets(profile.id).await.map_err(AppError::Internal)?;

    let addresses: Vec<String> = wallets.iter().map(|w| w.ordinals_address.clone()).collect();
    let wallet_entries: Vec<WalletEntry> = wallets
        .iter()
        .map(|w| WalletEntry {
            address: w.ordinals_address.clone(),
            label: Some(w.label.clone()),
        })
        .collect();

    let (bitmaps, traits, total, page, has_more) = run_multi_portfolio(
        &state,
        &addresses,
        query.page,
        query.limit,
        query.trait_filter.as_deref(),
    )
    .await?;

    Ok(Json(ProfilePortfolioResponse {
        profile_id: profile.id.to_string(),
        addresses: wallet_entries,
        bitmaps,
        traits,
        total,
        page,
        has_more,
    }))
}

/// GET /api/portfolio/profile/:profile_id — public; anyone can view an aggregated portfolio by profile ID
async fn get_portfolio_by_profile_id(
    State(state): State<AppState>,
    Path(profile_id): Path<Uuid>,
    Query(query): Query<PortfolioQuery>,
) -> Result<Json<ProfilePortfolioResponse>, AppError> {
    state
        .db
        .get_profile_by_id(profile_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("Profile not found".to_string()))?;

    let wallets = state.db.get_profile_wallets(profile_id).await.map_err(AppError::Internal)?;

    let addresses: Vec<String> = wallets.iter().map(|w| w.ordinals_address.clone()).collect();
    let wallet_entries: Vec<WalletEntry> = wallets
        .iter()
        .map(|w| WalletEntry {
            address: w.ordinals_address.clone(),
            label: Some(w.label.clone()),
        })
        .collect();

    let (bitmaps, traits, total, page, has_more) = run_multi_portfolio(
        &state,
        &addresses,
        query.page,
        query.limit,
        query.trait_filter.as_deref(),
    )
    .await?;

    Ok(Json(ProfilePortfolioResponse {
        profile_id: profile_id.to_string(),
        addresses: wallet_entries,
        bitmaps,
        traits,
        total,
        page,
        has_more,
    }))
}

/// Shared logic: fetch inscription IDs for all addresses, then query bitmaps + traits.
/// Returns (bitmaps, traits, total, page, has_more).
async fn run_multi_portfolio(
    state: &AppState,
    addresses: &[String],
    page: u64,
    limit: u64,
    trait_filter: Option<&str>,
) -> Result<(Vec<PortfolioBitmapItem>, Vec<TraitStat>, i64, u64, bool), AppError> {
    if addresses.is_empty() {
        return Ok((vec![], vec![], 0, page, false));
    }

    let limit = limit.clamp(1, 50) as i64;
    let offset = page as i64 * limit;

    let futs: Vec<_> = addresses
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

    let results: Vec<_> = stream::iter(futs)
        .buffer_unordered(10)
        .collect()
        .await;

    const MAX_INSCRIPTION_IDS: usize = 50_000;
    let mut inscription_to_owner: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut all_inscription_ids: Vec<String> = Vec::new();

    'outer: for result in results {
        let (addr, ids) = result.map_err(AppError::Internal)?;
        for id in ids {
            if all_inscription_ids.len() >= MAX_INSCRIPTION_IDS {
                break 'outer;
            }
            if !inscription_to_owner.contains_key(&id) {
                inscription_to_owner.insert(id.clone(), addr.clone());
                all_inscription_ids.push(id);
            }
        }
    }

    if all_inscription_ids.is_empty() {
        return Ok((vec![], vec![], 0, page, false));
    }

    let trait_future = state.db.get_trait_counts_by_inscription_ids(&all_inscription_ids);

    let (bitmaps, total, trait_raw) = if let Some(tf) = trait_filter {
        tokio::try_join!(
            state.db.get_bitmaps_by_inscription_ids_and_trait(&all_inscription_ids, tf, limit, offset),
            state.db.count_bitmaps_by_inscription_ids_and_trait(&all_inscription_ids, tf),
            trait_future,
        )
    } else {
        tokio::try_join!(
            state.db.get_bitmaps_by_inscription_ids(&all_inscription_ids, limit, offset),
            state.db.count_bitmaps_by_inscription_ids(&all_inscription_ids),
            trait_future,
        )
    }
    .map_err(AppError::Internal)?;

    let trait_stats: Vec<TraitStat> = trait_raw
        .into_iter()
        .map(|(name, count)| TraitStat { name, count })
        .collect();

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

    Ok((items, trait_stats, total, page, has_more))
}
