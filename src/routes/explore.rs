use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{errors::AppError, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/blocks", get(get_explore_blocks))
        .route("/blocks/:height", get(proxy_block_data))
        .route("/blocks/:height/meta", get(proxy_block_meta))
}

#[derive(Debug, Deserialize)]
pub struct ExploreQuery {
    pub filter: String,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_limit")]
    pub limit: u64,
}

fn default_limit() -> u64 {
    9
}

#[derive(Debug, Serialize)]
pub struct ExploreResponse {
    pub heights: Vec<u64>,
    pub has_more: bool,
    pub has_prev: bool,
    pub total: Option<u64>,
}

/// Maps filter parameter to database trait name
fn filter_to_trait(filter: &str) -> Option<&str> {
    match filter {
        "punks" => Some("punk"),
        "palindrome" => Some("palindrome"),
        "sub-100k" => Some("sub_100k"),
        "nakamoto" => Some("nakamoto"),
        "patoshi" => Some("patoshi"),
        "billionaire" => Some("billionaire"),
        "epic-sat" => Some("epic_sat"),
        "pizza" => Some("pizza"),
        "pristine-punk" => Some("pristine_punk"),
        "perfect-punk" => Some("perfect_punk"),
        "same-digits" => Some("same_digits"),
        _ => None,
    }
}

pub async fn get_explore_blocks(
    State(state): State<AppState>,
    Query(query): Query<ExploreQuery>,
) -> Result<Json<ExploreResponse>, AppError> {
    let limit = query.limit.clamp(1, 50) as i64;
    let page = query.page;
    let offset = page * query.limit;
    let offset_i64 = offset as i64;

    let has_prev = page > 0;

    // Get trait name from filter parameter
    let trait_name = match filter_to_trait(&query.filter) {
        Some(trait_name) => trait_name,
        None => return Err(AppError::BadRequest("Invalid filter category".to_string())),
    };

    // Query database for total count
    let total = state
        .db
        .count_bitmaps_by_trait(trait_name)
        .await
        .map_err(AppError::Internal)?;

    // Query database for block heights
    let heights_i64 = state
        .db
        .get_block_heights_by_trait(trait_name, limit, offset_i64)
        .await
        .map_err(AppError::Internal)?;

    let heights: Vec<u64> = heights_i64.into_iter().map(|h| h as u64).collect();
    let has_more = offset + (heights.len() as u64) < (total as u64);

    Ok(Json(ExploreResponse {
        heights,
        has_more,
        has_prev,
        total: Some(total as u64),
    }))
}

/// Proxy binary block data from the render API.
async fn proxy_block_data(
    State(state): State<AppState>,
    Path(height): Path<u64>,
) -> Result<Response, StatusCode> {
    let url = format!("{}/api/block/{}", state.render_api_base, height);

    let upstream = state
        .http_client
        .get(&url)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    if !upstream.status().is_success() {
        return Err(
            StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY)
        );
    }

    let bytes = upstream
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/octet-stream".parse().unwrap());
    headers.insert(
        "cache-control",
        "public, max-age=31536000, immutable".parse().unwrap(),
    );

    Ok((StatusCode::OK, headers, Body::from(bytes)).into_response())
}

/// Proxy JSON block metadata from the render API.
async fn proxy_block_meta(
    State(state): State<AppState>,
    Path(height): Path<u64>,
) -> Result<Response, StatusCode> {
    let url = format!("{}/api/block/{}/meta", state.render_api_base, height);

    let upstream = state
        .http_client
        .get(&url)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    if !upstream.status().is_success() {
        return Err(
            StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY)
        );
    }

    let bytes = upstream
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse().unwrap());
    headers.insert("cache-control", "public, max-age=60".parse().unwrap());

    Ok((StatusCode::OK, headers, Body::from(bytes)).into_response())
}
