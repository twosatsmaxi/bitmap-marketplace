use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            AppError::Internal(e) => {
                tracing::error!("Internal error: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            AppError::Database(e) => {
                tracing::error!("Database error: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            }
        };

        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::AppError;
    use axum::{body::Body, http::StatusCode, response::IntoResponse};
    use http_body_util::BodyExt;

    async fn body_string(body: Body) -> String {
        let bytes = body
            .collect()
            .await
            .expect("failed to collect body")
            .to_bytes();
        String::from_utf8(bytes.to_vec()).expect("body is not valid utf-8")
    }

    #[tokio::test]
    async fn unauthorized_error_returns_401_and_message() {
        let response = AppError::Unauthorized("invalid API key".to_string()).into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = body_string(response.into_body()).await;
        assert!(body.contains("invalid API key"));
        assert!(body.contains("\"error\""));
    }

    #[tokio::test]
    async fn internal_error_hides_underlying_message() {
        let response = AppError::Internal(anyhow::anyhow!("sensitive detail")).into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_string(response.into_body()).await;
        assert!(body.contains("Internal server error"));
        assert!(!body.contains("sensitive detail"));
    }

    #[tokio::test]
    async fn not_found_returns_404() {
        let response = AppError::NotFound("missing item".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_string(response.into_body()).await;
        assert!(body.contains("missing item"));
    }

    #[tokio::test]
    async fn bad_request_returns_400() {
        let response = AppError::BadRequest("bad input".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_string(response.into_body()).await;
        assert!(body.contains("bad input"));
    }

    #[tokio::test]
    async fn conflict_returns_409() {
        let response = AppError::Conflict("duplicate".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = body_string(response.into_body()).await;
        assert!(body.contains("duplicate"));
    }

    #[tokio::test]
    async fn database_error_hides_details() {
        // Construct a sqlx error by using ColumnNotFound (doesn't need a DB connection)
        let sqlx_err = sqlx::Error::ColumnNotFound("secret_column".to_string());
        let response = AppError::Database(sqlx_err).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_string(response.into_body()).await;
        assert!(body.contains("Database error"));
        assert!(!body.contains("secret_column"));
    }
}
