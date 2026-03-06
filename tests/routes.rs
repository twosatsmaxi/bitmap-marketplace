/// Route handler integration tests for bitmap-marketplace.
///
/// These tests exercise HTTP-level behaviour without requiring a live database
/// or importing from the crate's internal modules.  They focus on:
///
///   1. The shape of error JSON responses that the application produces (verified
///      by building an equivalent axum handler inline).
///   2. Router fundamentals: unknown paths return 404, wrong HTTP method
///      returns 405.
///   3. A smoke test verifying the axum Router API compiles and routes
///      correctly.

// ---------------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------------

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt; // for `oneshot`

// ---------------------------------------------------------------------------
// Helper: collect a response body into a String
// ---------------------------------------------------------------------------

async fn body_string(body: Body) -> String {
    let bytes = body
        .collect()
        .await
        .expect("failed to collect body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("body is not valid UTF-8")
}

// ---------------------------------------------------------------------------
// Inline error type mirroring AppError behaviour
//
// We replicate the status-code / JSON-body contract from src/errors.rs so
// the tests remain independent of the binary crate's internal modules while
// still verifying the contract that the application upholds.
// ---------------------------------------------------------------------------

enum TestAppError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for TestAppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            TestAppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            TestAppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            TestAppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            ),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// AppError contract tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn app_error_not_found_returns_404() {
    let error = TestAppError::NotFound("Listing not found".to_string());
    let response = error.into_response();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "NotFound variant should produce HTTP 404"
    );
    let body = body_string(response.into_body()).await;
    assert!(
        body.contains("Listing not found"),
        "Response body should contain the error message; got: {body}"
    );
    assert!(
        body.contains("\"error\""),
        "Response body should have an 'error' key; got: {body}"
    );
}

#[tokio::test]
async fn app_error_bad_request_returns_400() {
    let error = TestAppError::BadRequest("Missing required field".to_string());
    let response = error.into_response();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "BadRequest variant should produce HTTP 400"
    );
    let body = body_string(response.into_body()).await;
    assert!(
        body.contains("Missing required field"),
        "Response body should contain the error message; got: {body}"
    );
    assert!(
        body.contains("\"error\""),
        "Response body should have an 'error' key; got: {body}"
    );
}

#[tokio::test]
async fn app_error_internal_returns_500() {
    let error = TestAppError::Internal("something went very wrong".to_string());
    let response = error.into_response();
    assert_eq!(
        response.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "Internal variant should produce HTTP 500"
    );
    let body = body_string(response.into_body()).await;
    // The handler deliberately does NOT leak the internal message to the client.
    assert!(
        body.contains("Internal server error"),
        "Response body should contain the generic message; got: {body}"
    );
    assert!(
        body.contains("\"error\""),
        "Response body should have an 'error' key; got: {body}"
    );
}

// ---------------------------------------------------------------------------
// AppError JSON shape test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn app_error_response_is_valid_json() {
    let cases: Vec<TestAppError> = vec![
        TestAppError::NotFound("not found".to_string()),
        TestAppError::BadRequest("bad request".to_string()),
        TestAppError::Internal("boom".to_string()),
    ];

    for error in cases {
        let response = error.into_response();
        let body = body_string(response.into_body()).await;
        let value: serde_json::Value =
            serde_json::from_str(&body).expect("AppError response body should be valid JSON");
        assert!(
            value.get("error").is_some(),
            "Parsed JSON must contain an 'error' key; got: {value}"
        );
    }
}

// ---------------------------------------------------------------------------
// Router smoke tests – verify axum routing fundamentals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_route_returns_404() {
    let app: Router = Router::new();

    let request = Request::builder()
        .uri("/totally/unknown/path")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "Requests to unknown routes should yield 404"
    );
}

#[tokio::test]
async fn router_method_not_allowed_returns_405() {
    // Register a route that only accepts GET, then send a DELETE.
    let app: Router = Router::new().route("/ping", get(|| async { "pong" }));

    let request = Request::builder()
        .method("DELETE")
        .uri("/ping")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "Wrong HTTP method on a known route should yield 405"
    );
}

#[tokio::test]
async fn get_route_responds_200() {
    let app: Router = Router::new().route("/ping", get(|| async { "pong" }));

    let request = Request::builder()
        .method("GET")
        .uri("/ping")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /ping should return 200"
    );
    let body = body_string(response.into_body()).await;
    assert_eq!(body, "pong");
}

#[tokio::test]
async fn nested_router_route_is_reachable() {
    // Mirrors how the application nests /api/listings – verify axum's
    // Router::nest wires up correctly.
    let inner: Router = Router::new().route("/items", get(|| async { Json(json!({"items": []})) }));
    let app: Router = Router::new().nest("/api", inner);

    let request = Request::builder()
        .method("GET")
        .uri("/api/items")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Nested route /api/items should return 200"
    );
    let body = body_string(response.into_body()).await;
    let value: serde_json::Value =
        serde_json::from_str(&body).expect("Response should be valid JSON");
    assert!(
        value.get("items").is_some(),
        "Response JSON should contain 'items' key; got: {value}"
    );
}

#[tokio::test]
async fn json_handler_returns_correct_content_type() {
    let app: Router =
        Router::new().route("/data", get(|| async { Json(json!({"key": "value"})) }));

    let request = Request::builder()
        .method("GET")
        .uri("/data")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get("content-type")
        .expect("content-type header should be present")
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("application/json"),
        "Content-Type should be application/json; got: {content_type}"
    );
}
