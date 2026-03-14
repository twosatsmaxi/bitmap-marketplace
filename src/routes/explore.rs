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

pub async fn get_explore_blocks(
    State(_state): State<AppState>,
    Query(query): Query<ExploreQuery>,
) -> Result<Json<ExploreResponse>, AppError> {
    let limit = query.limit.clamp(1, 50);
    let page = query.page;
    let offset = page * limit;

    let has_prev = page > 0;
    let mut total = None;
    let mut heights = Vec::new();
    let mut has_more = false;

    match query.filter.as_str() {
        "punks" => {
            // heights where height % 4 == 3
            // Valid heights are: 3, 7, 11, 15...
            // the nth element is: 3 + n * 4
            let start_n = offset;
            for i in 0..limit {
                heights.push(3 + (start_n + i) * 4);
            }
            has_more = true;
        }
        "palindrome" => {
            // heights whose decimal string is a palindrome
            // This is infinite, but we need to generate them.
            // A naive generator counting from 0 upwards:
            let mut current = 0;
            let mut count = 0;
            while count < offset + limit + 1 {
                let s = current.to_string();
                let reversed: String = s.chars().rev().collect();
                if s == reversed {
                    if count >= offset && count < offset + limit {
                        heights.push(current);
                    }
                    if count == offset + limit {
                        has_more = true;
                    }
                    count += 1;
                }
                current += 1;
            }
        }
        "sub-100k" => {
            // heights 0..99_999
            let total_count = 100_000;
            total = Some(total_count);
            for i in 0..limit {
                let h = offset + i;
                if h < total_count {
                    heights.push(h);
                }
            }
            has_more = offset + limit < total_count;
        }
        "nakamoto" => {
            // heights 0..36_288
            let total_count = 36_289;
            total = Some(total_count);
            for i in 0..limit {
                let h = offset + i;
                if h < total_count {
                    heights.push(h);
                }
            }
            has_more = offset + limit < total_count;
        }
        "patoshi" | "billionaire" | "epic-sat" | "pizza" | "pristine-punk" | "perfect-punk" => {
            // Curated lists - hardcoded stubs
            let stub_data = match query.filter.as_str() {
                "patoshi" => vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                "billionaire" => vec![1000, 2000, 3000],
                "epic-sat" => vec![100000, 200000],
                "pizza" => vec![57043],
                "pristine-punk" => vec![3, 7, 11],
                "perfect-punk" => vec![15, 19, 23],
                _ => vec![],
            };
            total = Some(stub_data.len() as u64);
            for i in 0..limit {
                let idx = (offset + i) as usize;
                if let Some(&h) = stub_data.get(idx) {
                    heights.push(h);
                }
            }
            has_more = offset + limit < stub_data.len() as u64;
        }
        _ => {
            return Err(AppError::BadRequest("Invalid filter category".to_string()));
        }
    }

    Ok(Json(ExploreResponse {
        heights,
        has_more,
        has_prev,
        total,
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
        return Err(StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY));
    }

    let bytes = upstream.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;

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
        return Err(StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY));
    }

    let bytes = upstream.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;

    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse().unwrap());
    headers.insert("cache-control", "public, max-age=60".parse().unwrap());

    Ok((StatusCode::OK, headers, Body::from(bytes)).into_response())
}
