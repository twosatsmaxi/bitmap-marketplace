use crate::{
    errors::{AppError, AppResult},
    AppState,
};
use axum::{
    body::Body,
    extract::{Path, State},
    response::Response,
    routing::get,
    Json, Router,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:inscription_id", get(get_inscription))
        .route("/:inscription_id/history", get(get_inscription_history))
        .route("/:inscription_id/content", get(get_inscription_content))
}

async fn get_inscription(
    State(state): State<AppState>,
    Path(inscription_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let inscription = state
        .db
        .get_inscription(&inscription_id)
        .await
        .map_err(AppError::Internal)?;
    match inscription {
        Some(i) => Ok(Json(serde_json::json!(i))),
        None => Err(AppError::NotFound("Inscription not found".to_string())),
    }
}

async fn get_inscription_history(
    State(state): State<AppState>,
    Path(inscription_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let activity = state
        .db
        .get_activity_by_inscription(&inscription_id, 50, 0)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "history": activity })))
}

async fn get_inscription_content(
    Path(inscription_id): Path<String>,
) -> Result<Response<Body>, AppError> {
    let ord = crate::services::ord::OrdClient::new();
    let bytes = ord
        .get_inscription_content(&inscription_id)
        .await
        .map_err(AppError::Internal)?;

    let response = Response::builder()
        .header("content-type", "application/octet-stream")
        .body(Body::from(bytes))
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Failed to build response: {}", e)))?;

    Ok(response)
}
