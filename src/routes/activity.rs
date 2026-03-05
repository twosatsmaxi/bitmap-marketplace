use axum::{extract::{State, Query}, routing::get, Json, Router};
use crate::{errors::{AppError, AppResult}, AppState};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_activity))
}

#[derive(Deserialize)]
struct Pagination {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn get_activity(
    State(state): State<AppState>,
    Query(pagination): Query<Pagination>,
) -> AppResult<Json<serde_json::Value>> {
    let limit = pagination.limit.unwrap_or(50);
    let offset = pagination.offset.unwrap_or(0);
    let activity = state.db.get_activity_feed(limit, offset).await.map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "activity": activity })))
}