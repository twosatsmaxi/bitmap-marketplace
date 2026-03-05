use axum::{routing::post, Json, Router};
use crate::{errors::AppResult, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/buy", post(buy))
        .route("/offer", post(make_offer))
}

async fn buy(Json(body): Json<serde_json::Value>) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "order": body })))
}

async fn make_offer(Json(body): Json<serde_json::Value>) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "offer": body })))
}
