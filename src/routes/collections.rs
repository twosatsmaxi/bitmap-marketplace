use axum::{extract::{Path, State, Query}, routing::get, Json, Router};
use crate::{errors::{AppError, AppResult}, AppState};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_collections))
        .route("/:slug", get(get_collection))
        .route("/:slug/stats", get(get_collection_stats))
        .route("/:slug/inscriptions", get(get_collection_inscriptions))
}

async fn list_collections(
    State(state): State<AppState>,
) -> AppResult<Json<serde_json::Value>> {
    let collections = state.db.list_collections().await.map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "collections": collections })))
}

async fn get_collection(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let collection = state.db.get_collection_by_slug(&slug).await.map_err(AppError::Internal)?;
    match collection {
        Some(c) => Ok(Json(serde_json::json!(c))),
        None => Err(AppError::NotFound("Collection not found".to_string())),
    }
}

async fn get_collection_stats(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let collection = state.db.get_collection_by_slug(&slug).await.map_err(AppError::Internal)?;
    let collection = collection.ok_or_else(|| AppError::NotFound("Collection not found".to_string()))?;
    
    let stats = state.db.get_collection_stats(collection.id).await.map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!(stats)))
}

#[derive(Deserialize)]
struct Pagination {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn get_collection_inscriptions(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(pagination): Query<Pagination>,
) -> AppResult<Json<serde_json::Value>> {
    let collection = state.db.get_collection_by_slug(&slug).await.map_err(AppError::Internal)?;
    let collection = collection.ok_or_else(|| AppError::NotFound("Collection not found".to_string()))?;
    
    let limit = pagination.limit.unwrap_or(50);
    let offset = pagination.offset.unwrap_or(0);
    
    let inscriptions = state.db.get_inscriptions_by_collection(collection.id, limit, offset).await.map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "inscriptions": inscriptions })))
}