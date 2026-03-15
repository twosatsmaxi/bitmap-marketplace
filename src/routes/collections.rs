use crate::{
    errors::{AppError, AppResult},
    AppState,
};
use axum::{
    http::HeaderMap,
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_collections))
        .route("/:slug", get(get_collection))
        .route("/:slug/stats", get(get_collection_stats))
        .route(
            "/:slug/inscriptions",
            get(get_collection_inscriptions).post(assign_collection_inscriptions),
        )
}

async fn list_collections(State(state): State<AppState>) -> AppResult<Json<serde_json::Value>> {
    let collections = state
        .db
        .list_collections()
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "collections": collections })))
}

async fn get_collection(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let collection = state
        .db
        .get_collection_by_slug(&slug)
        .await
        .map_err(AppError::Internal)?;
    match collection {
        Some(c) => Ok(Json(serde_json::json!(c))),
        None => Err(AppError::NotFound("Collection not found".to_string())),
    }
}

async fn get_collection_stats(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let collection = state
        .db
        .get_collection_by_slug(&slug)
        .await
        .map_err(AppError::Internal)?;
    let collection =
        collection.ok_or_else(|| AppError::NotFound("Collection not found".to_string()))?;

    let stats = state
        .db
        .get_collection_stats(collection.id)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!(stats)))
}

#[derive(Deserialize)]
struct Pagination {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Deserialize)]
struct AssignInscriptionsRequest {
    inscription_ids: Vec<String>,
}

async fn get_collection_inscriptions(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(pagination): Query<Pagination>,
) -> AppResult<Json<serde_json::Value>> {
    let collection = state
        .db
        .get_collection_by_slug(&slug)
        .await
        .map_err(AppError::Internal)?;
    let collection =
        collection.ok_or_else(|| AppError::NotFound("Collection not found".to_string()))?;

    let limit = pagination.limit.unwrap_or(50);
    let offset = pagination.offset.unwrap_or(0);

    let inscriptions = state
        .db
        .get_inscriptions_by_collection(collection.id, limit, offset)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({ "inscriptions": inscriptions })))
}

async fn assign_collection_inscriptions(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(req): Json<AssignInscriptionsRequest>,
) -> AppResult<Json<serde_json::Value>> {
    require_admin_api_key(&headers)?;

    if req.inscription_ids.is_empty() {
        return Err(AppError::BadRequest(
            "inscription_ids must not be empty".to_string(),
        ));
    }

    let collection = state
        .db
        .get_collection_by_slug(&slug)
        .await
        .map_err(AppError::Internal)?;
    let collection =
        collection.ok_or_else(|| AppError::NotFound("Collection not found".to_string()))?;

    let updated_count = state
        .db
        .assign_inscriptions_to_collection(collection.id, &req.inscription_ids)
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "collection_id": collection.id,
        "updated_count": updated_count,
    })))
}

fn require_admin_api_key(headers: &HeaderMap) -> AppResult<()> {
    let configured_key = std::env::var("ADMIN_API_KEY").map_err(|_| {
        AppError::Internal(anyhow::anyhow!(
            "ADMIN_API_KEY must be configured for collection assignment"
        ))
    })?;

    let provided_key = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("missing x-api-key header".to_string()))?;

    if provided_key != configured_key {
        return Err(AppError::Unauthorized("invalid API key".to_string()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::require_admin_api_key;
    use axum::http::{HeaderMap, HeaderValue};
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn require_admin_api_key_rejects_missing_header() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("ADMIN_API_KEY", "secret");

        let err = require_admin_api_key(&HeaderMap::new()).unwrap_err();
        let message = match err {
            crate::errors::AppError::Unauthorized(message) => message,
            other => panic!("expected unauthorized error, got {other:?}"),
        };
        assert!(message.contains("missing x-api-key"));
    }

    #[test]
    fn require_admin_api_key_rejects_invalid_header() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("ADMIN_API_KEY", "secret");

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("wrong"));

        let err = require_admin_api_key(&headers).unwrap_err();
        let message = match err {
            crate::errors::AppError::Unauthorized(message) => message,
            other => panic!("expected unauthorized error, got {other:?}"),
        };
        assert!(message.contains("invalid API key"));
    }

    #[test]
    fn require_admin_api_key_accepts_matching_header() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("ADMIN_API_KEY", "secret");

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("secret"));

        require_admin_api_key(&headers).expect("matching key should succeed");
    }
}
