use axum::{routing::get, Json, Router};
use crate::{errors::AppResult, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_activity))
}

async fn get_activity() -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "activity": [] })))
}
