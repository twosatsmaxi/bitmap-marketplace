use axum::{
    extract::Path,
    routing::{get, post, delete},
    Json, Router,
};
use crate::{errors::AppResult, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_listings).post(create_listing))
        .route("/:id", get(get_listing).delete(cancel_listing))
}

async fn list_listings() -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "listings": [] })))
}

async fn create_listing(Json(body): Json<serde_json::Value>) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "listing": body })))
}

async fn get_listing(Path(id): Path<String>) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn cancel_listing(Path(id): Path<String>) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "cancelled": id })))
}
