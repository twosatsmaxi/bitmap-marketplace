use axum::{extract::Path, routing::get, Json, Router};
use crate::{errors::AppResult, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:inscription_id", get(get_inscription))
}

async fn get_inscription(Path(inscription_id): Path<String>) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "inscription_id": inscription_id })))
}
