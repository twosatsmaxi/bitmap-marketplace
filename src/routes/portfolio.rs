use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        header::{CACHE_CONTROL, ETAG, IF_NONE_MATCH},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{errors::AppError, routes::auth::AuthenticatedUser, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mine", get(get_my_portfolio))
        .route("/mine/bitfield", get(get_my_bitfield))
        .route("/profile/:profile_id", get(get_portfolio_by_profile_id))
        .route("/profile/:profile_id/bitfield", get(get_profile_bitfield))
        .route("/:address/bitfield", get(get_portfolio_bitfield))
        .route("/:address", get(get_portfolio))
}

// ---------------------------------------------------------------------------
// ETag helpers
// ---------------------------------------------------------------------------

const CACHE_PRIVATE: &str = "private, max-age=600";
const CACHE_PUBLIC: &str = "public, max-age=600, s-maxage=3600, stale-while-revalidate=60";

/// Generates a weak ETag from (address, outputs_count) pairs plus query params.
/// Sorted by address for stability. Busts when wallets are added/removed,
/// when outputs change, or when the query view changes (wallet/trait/page/limit).
fn profile_etag(address_counts: &[(String, u64)], query: &PortfolioQuery) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut sorted = address_counts.to_vec();
    sorted.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut h = DefaultHasher::new();
    sorted.hash(&mut h);
    query.page.hash(&mut h);
    query.limit.hash(&mut h);
    query.trait_filter.hash(&mut h);
    query.wallet.hash(&mut h);
    format!("W/\"{:016x}\"", h.finish())
}

/// Returns true if the request's If-None-Match header matches the given ETag.
fn etag_matches(req_headers: &HeaderMap, etag: &str) -> bool {
    req_headers
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|v| v == etag || v == "*")
        .unwrap_or(false)
}

/// Builds ETag + Cache-Control headers for a response.
fn cache_headers(etag: &str, cache_control: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        ETAG,
        HeaderValue::from_str(etag).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    headers
}

/// Builds a 304 Not Modified response with ETag + Cache-Control headers.
fn not_modified(etag: &str, cache_control: &'static str) -> Response {
    (StatusCode::NOT_MODIFIED, cache_headers(etag, cache_control)).into_response()
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PortfolioQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_limit")]
    pub limit: u64,
    /// Filter by trait (optional)
    pub trait_filter: Option<String>,
    /// Filter by wallet address (optional)
    pub wallet: Option<String>,
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

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn get_portfolio(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(query): Query<PortfolioQuery>,
) -> Result<Json<PortfolioResponse>, AppError> {
    let limit = query.limit.clamp(1, 50) as i64;
    let page = query.page;
    let offset = page as i64 * limit;

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

/// GET /api/portfolio/mine — authenticated; private cache (10 min) + ETag revalidation
async fn get_my_portfolio(
    AuthenticatedUser(profile): AuthenticatedUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PortfolioQuery>,
) -> Result<Response, AppError> {
    let wallets = state
        .db
        .get_profile_wallets(profile.id)
        .await
        .map_err(AppError::Internal)?;

    let addresses: Vec<String> = wallets.iter().map(|w| w.ordinals_address.clone()).collect();
    let address_counts = fetch_address_counts(&state, &addresses).await?;
    let etag = profile_etag(&address_counts, &query);

    if etag_matches(&headers, &etag) {
        return Ok(not_modified(&etag, CACHE_PRIVATE));
    }

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
        query.wallet.as_deref(),
    )
    .await?;

    Ok((
        cache_headers(&etag, CACHE_PRIVATE),
        Json(ProfilePortfolioResponse {
            profile_id: profile.id.to_string(),
            addresses: wallet_entries,
            bitmaps,
            traits,
            total,
            page,
            has_more,
        }),
    )
        .into_response())
}

/// GET /api/portfolio/profile/:profile_id — public; edge-cacheable with ETag revalidation
async fn get_portfolio_by_profile_id(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(profile_id): Path<Uuid>,
    Query(query): Query<PortfolioQuery>,
) -> Result<Response, AppError> {
    state
        .db
        .get_profile_by_id(profile_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("Profile not found".to_string()))?;

    let wallets = state
        .db
        .get_profile_wallets(profile_id)
        .await
        .map_err(AppError::Internal)?;

    let addresses: Vec<String> = wallets.iter().map(|w| w.ordinals_address.clone()).collect();
    let etag_addresses: Vec<String> = if let Some(ref wallet) = query.wallet {
        addresses.iter().filter(|a| *a == wallet).cloned().collect()
    } else {
        addresses.clone()
    };
    let address_counts = fetch_address_counts(&state, &etag_addresses).await?;
    let etag = profile_etag(&address_counts, &query);

    if etag_matches(&headers, &etag) {
        return Ok(not_modified(&etag, CACHE_PUBLIC));
    }

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
        query.wallet.as_deref(),
    )
    .await?;

    Ok((
        cache_headers(&etag, CACHE_PUBLIC),
        Json(ProfilePortfolioResponse {
            profile_id: profile_id.to_string(),
            addresses: wallet_entries,
            bitmaps,
            traits,
            total,
            page,
            has_more,
        }),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Shared fetch logic
// ---------------------------------------------------------------------------

/// Fetch outputs_count for each address via GET /address/{addr}/lite in parallel.
/// Returns sorted (address, count) pairs for ETag computation.
async fn fetch_address_counts(
    state: &AppState,
    addresses: &[String],
) -> Result<Vec<(String, u64)>, AppError> {
    let futs: Vec<_> = addresses
        .iter()
        .map(|addr| {
            let client = state.ord_client.clone();
            let addr = addr.clone();
            async move {
                let count = client.get_address_outputs_count(&addr).await?;
                Ok::<_, anyhow::Error>((addr, count))
            }
        })
        .collect();

    stream::iter(futs)
        .buffer_unordered(10)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|r| r.map_err(AppError::Internal))
        .collect()
}

/// Fetch inscription IDs for all addresses, then query bitmaps + traits.
/// Returns (bitmaps, traits, total, page, has_more).
async fn run_multi_portfolio(
    state: &AppState,
    addresses: &[String],
    page: u64,
    limit: u64,
    trait_filter: Option<&str>,
    wallet_filter: Option<&str>,
) -> Result<(Vec<PortfolioBitmapItem>, Vec<TraitStat>, i64, u64, bool), AppError> {
    if addresses.is_empty() {
        return Ok((vec![], vec![], 0, page, false));
    }

    let limit = limit.clamp(1, 50) as i64;
    let offset = page as i64 * limit;

    // If a wallet filter is set, only fetch from that one address
    let addresses_to_fetch: &[String] = if let Some(wallet) = wallet_filter {
        if let Some(pos) = addresses.iter().position(|a| a == wallet) {
            &addresses[pos..=pos]
        } else {
            return Ok((vec![], vec![], 0, page, false));
        }
    } else {
        addresses
    };

    let futs: Vec<_> = addresses_to_fetch
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

    // Filter to a single wallet if requested
    if let Some(wallet) = wallet_filter {
        all_inscription_ids.retain(|id| {
            inscription_to_owner.get(id).map(|o| o == wallet).unwrap_or(false)
        });
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

// ---------------------------------------------------------------------------
// Bitfield endpoints
// ---------------------------------------------------------------------------

fn bitfield_response(bitfield: Vec<u8>, total_blocks: usize, owned_count: usize, cache_control: &'static str) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/octet-stream"));
    headers.insert("x-total-blocks", HeaderValue::from_str(&total_blocks.to_string()).unwrap());
    headers.insert("x-owned-count", HeaderValue::from_str(&owned_count.to_string()).unwrap());
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    (StatusCode::OK, headers, Body::from(bitfield)).into_response()
}

async fn build_bitfield(
    state: &AppState,
    addresses: &[String],
) -> Result<(Vec<u8>, usize, usize), AppError> {
    // Gather inscription IDs across all addresses in parallel
    let futs: Vec<_> = addresses
        .iter()
        .map(|addr| {
            let client = state.ord_client.clone();
            let addr = addr.clone();
            async move { client.get_address_inscription_ids(&addr).await }
        })
        .collect();

    let results: Vec<_> = stream::iter(futs)
        .buffer_unordered(10)
        .collect()
        .await;

    let mut all_inscription_ids: Vec<String> = Vec::new();
    for result in results {
        let ids = result.map_err(AppError::Internal)?;
        all_inscription_ids.extend(ids);
    }
    all_inscription_ids.sort_unstable();
    all_inscription_ids.dedup();

    if all_inscription_ids.is_empty() {
        return Ok((vec![], 0, 0));
    }

    let heights = state
        .db
        .get_block_heights_by_inscription_ids(&all_inscription_ids)
        .await
        .map_err(AppError::Internal)?;

    let max_height = state
        .db
        .get_max_block_height()
        .await
        .map_err(AppError::Internal)?;
    let total_blocks = (max_height + 1) as usize;

    let byte_count = (total_blocks + 7) / 8;
    let mut bitfield = vec![0u8; byte_count];
    for &h in &heights {
        let h = h as usize;
        if h < total_blocks {
            bitfield[h >> 3] |= 1 << (h & 7);
        }
    }

    Ok((bitfield, total_blocks, heights.len()))
}

async fn get_portfolio_bitfield(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Response, AppError> {
    let (bitfield, total, owned) = build_bitfield(&state, &[address]).await?;
    Ok(bitfield_response(bitfield, total, owned, "public, max-age=60, stale-while-revalidate=30"))
}

async fn get_profile_bitfield(
    State(state): State<AppState>,
    Path(profile_id): Path<Uuid>,
) -> Result<Response, AppError> {
    state
        .db
        .get_profile_by_id(profile_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("Profile not found".to_string()))?;

    let wallets = state
        .db
        .get_profile_wallets(profile_id)
        .await
        .map_err(AppError::Internal)?;
    let addresses: Vec<String> = wallets.iter().map(|w| w.ordinals_address.clone()).collect();
    let (bitfield, total, owned) = build_bitfield(&state, &addresses).await?;
    Ok(bitfield_response(bitfield, total, owned, CACHE_PUBLIC))
}

async fn get_my_bitfield(
    AuthenticatedUser(profile): AuthenticatedUser,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    let wallets = state
        .db
        .get_profile_wallets(profile.id)
        .await
        .map_err(AppError::Internal)?;
    let addresses: Vec<String> = wallets.iter().map(|w| w.ordinals_address.clone()).collect();
    let (bitfield, total, owned) = build_bitfield(&state, &addresses).await?;
    Ok(bitfield_response(bitfield, total, owned, CACHE_PRIVATE))
}
