use axum::{extract::Path, routing::get, Json, Router};
use crate::{errors::AppResult, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_collections))
        .route("/:slug", get(get_collection))
        .route("/:slug/stats", get(get_collection_stats))
        .route("/:slug/inscriptions", get(get_collection_inscriptions))
}

async fn list_collections() -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "collections": [] })))
}

async fn get_collection(Path(slug): Path<String>) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "slug": slug })))
}

async fn get_collection_stats(Path(slug): Path<String>) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "slug": slug, "stats": {} })))
}

async fn get_collection_inscriptions(Path(slug): Path<String>) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "slug": slug, "inscriptions": [] })))
}
