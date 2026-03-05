use axum::{extract::{Path, State}, routing::get, Json, Router};
use crate::{errors::{AppError, AppResult}, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:inscription_id", get(get_inscription))
}

async fn get_inscription(
    State(state): State<AppState>,
    Path(inscription_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let inscription = state.db.get_inscription(&inscription_id).await.map_err(AppError::Internal)?;
    match inscription {
        Some(i) => Ok(Json(serde_json::json!(i))),
        None => Err(AppError::NotFound("Inscription not found".to_string())),
    }
}