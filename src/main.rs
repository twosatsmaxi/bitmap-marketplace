use anyhow::Result;
use axum::extract::State;
use axum::http::{
    header::{AUTHORIZATION, CONTENT_TYPE},
    Method, StatusCode,
};
use axum::routing::get;
use axum::Router;
use bitcoin::Network;
use secrecy::SecretString;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tower::limit::ConcurrencyLimitLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::GovernorLayer;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{self, TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod db;
pub mod errors;
mod models;
mod routes;
mod services;
pub mod ws;

use crate::db::Database;
use crate::services::marketplace_keypair::MarketplaceKeypair;
use crate::services::ord::OrdClient;

/// Maximum number of pending auth challenges to store
const MAX_CHALLENGES: u64 = 10_000;

/// Challenge nonces expire after this duration
const CHALLENGE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub ws_broadcaster: Arc<ws::WsBroadcaster>,
    pub marketplace_keypair: Arc<MarketplaceKeypair>,
    pub http_client: reqwest::Client,
    pub ord_client: OrdClient,
    pub render_api_base: String,
    pub network: Network,
    pub allowed_address_network: Network,
    pub jwt_secret: SecretString,
    pub marketplace_fee_address: Option<String>,
    pub marketplace_fee_bps: u64,
    pub challenges: moka::future::Cache<String, routes::auth::Challenge>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "bitmap_marketplace=debug,info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db = Database::new()?;
    match tokio::time::timeout(Duration::from_secs(5), db.run_migrations()).await {
        Ok(Ok(())) => tracing::info!("DB migrations applied successfully"),
        Ok(Err(e)) => tracing::warn!("DB migrations skipped (migration error): {e}"),
        Err(_) => tracing::warn!("DB migrations skipped (connection timed out)"),
    }

    // Spawn background services (disabled for now)
    /*
    {
        let db_clone = db.clone();
        tokio::spawn(async move {
            if let Ok(watcher) = crate::services::mempool_watcher::MempoolWatcher::new(db_clone) {
                if let Err(e) = watcher.run().await {
                    tracing::error!("Mempool watcher error: {}", e);
                }
            }
        });
    }
    {
        let db_clone = db.clone();
        tokio::spawn(async move {
            if let Ok(indexer) = crate::services::indexer::InscriptionIndexer::new(db_clone) {
                if let Err(e) = indexer.run().await {
                    tracing::error!("Indexer error: {}", e);
                }
            }
        });
    }
    {
        let db_clone = db.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.tick().await;

            loop {
                interval.tick().await;

                match db_clone.expire_stale_offers().await {
                    Ok(expired_count) if expired_count > 0 => {
                        tracing::info!("Expired {} stale offers", expired_count);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!("Expired offer cleanup error: {}", e);
                    }
                }
            }
        });
    }
    */

    let ws_broadcaster = Arc::new(ws::WsBroadcaster::new());

    let marketplace_keypair = MarketplaceKeypair::from_env()
        .expect("MarketplaceKeypair: failed to load from env (check MARKETPLACE_SECRET_KEY)");
    let network = std::env::var("BITCOIN_NETWORK")
        .ok()
        .and_then(|value| Network::from_str(&value).ok())
        .unwrap_or(Network::Regtest);

    // Network to validate addresses against in auth flow (defaults to mainnet)
    let allowed_address_network = std::env::var("ALLOWED_ADDRESS_NETWORK")
        .ok()
        .and_then(|value| Network::from_str(&value).ok())
        .unwrap_or(Network::Bitcoin);

    let render_api_base =
        std::env::var("RENDER_API_BASE").unwrap_or_else(|_| "http://r2d2.local:3020".to_string());

    let jwt_secret = SecretString::new(
        std::env::var("JWT_SECRET")
            .expect("JWT_SECRET must be set (use a strong random secret in production)"),
    );

    let frontend_url = std::env::var("FRONTEND_URL").ok();

    let marketplace_fee_address = std::env::var("MARKETPLACE_FEE_ADDRESS").ok();
    let marketplace_fee_bps: u64 = std::env::var("MARKETPLACE_FEE_BPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let state = AppState {
        db,
        ws_broadcaster: ws_broadcaster.clone(),
        marketplace_keypair,
        http_client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .user_agent("bitmap-marketplace/0.1")
            .build()
            .expect("failed to build HTTP client"),
        ord_client: OrdClient::new(),
        render_api_base,
        network,
        allowed_address_network,
        jwt_secret,
        marketplace_fee_address,
        marketplace_fee_bps,
        challenges: moka::future::Cache::builder()
            .max_capacity(MAX_CHALLENGES)
            .time_to_live(CHALLENGE_TTL)
            .build(),
    };

    // Health check endpoint — unauthenticated, not rate-limited, for load balancers / k8s probes.
    async fn health_handler(State(s): State<AppState>) -> impl axum::response::IntoResponse {
        match sqlx::query("SELECT 1").execute(&s.db.pool).await {
            Ok(_) => StatusCode::OK,
            Err(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    // Per-IP rate limiting config.
    // Uses PeerIpKeyExtractor (TCP peer address) — correct for direct deployments with no
    // reverse proxy. If deployed behind a load balancer/proxy later, switch to
    // SmartIpKeyExtractor and configure trusted proxy allowlisting to prevent header spoofing.
    // 300-burst / 100s-refill: generous for legitimate marketplace users with batch-heavy frontends.
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(100)
            .burst_size(300)
            .finish()
            .unwrap(),
    );

    // Stricter rate limit for auth endpoints: 3 req/s, 10 burst
    let auth_governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(3)
            .burst_size(10)
            .finish()
            .unwrap(),
    );

    // Spawn background task to evict stale rate-limit entries (prevents unbounded memory growth).
    let governor_conf_cleanup = Arc::clone(&governor_conf);
    let auth_governor_conf_cleanup = Arc::clone(&auth_governor_conf);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            governor_conf_cleanup.limiter().retain_recent();
            auth_governor_conf_cleanup.limiter().retain_recent();
        }
    });

    // /api/auth subrouter — stricter rate limiting to prevent brute-force attacks
    let auth_api_router = routes::auth_router()
        .layer(GovernorLayer {
            config: Arc::clone(&auth_governor_conf),
        })
        .layer(ConcurrencyLimitLayer::new(100))
        .layer(RequestBodyLimitLayer::new(512 * 1024))
        .layer(TimeoutLayer::new(Duration::from_secs(30)));

    // /api subrouter — full hardening stack
    // Bitcoin RPC calls can take up to 10s, so we use 30s timeout to be safe.
    let api_router = routes::router()
        .layer(GovernorLayer {
            config: Arc::clone(&governor_conf),
        })
        .layer(ConcurrencyLimitLayer::new(500))
        .layer(RequestBodyLimitLayer::new(512 * 1024)) // 512KB explicit limit
        .layer(TimeoutLayer::new(Duration::from_secs(30)));

    // /ws subrouter — NO timeout, NO body limit, NO concurrency limit, NO rate limit.
    // WebSocket upgrade holds connections open indefinitely; these layers would kill live sockets.
    let ws_router = ws::router(ws_broadcaster);

    let app = Router::new()
        .route("/health", get(health_handler))
        .nest("/api/auth", auth_api_router)
        .nest("/api", api_router)
        .merge(ws_router)
        .layer(CompressionLayer::new())
        .layer(
            TraceLayer::new_for_http()
                .on_response(
                    trace::DefaultOnResponse::new()
                        .level(tracing::Level::DEBUG),
                )
                .on_request(
                    trace::DefaultOnRequest::new()
                        .level(tracing::Level::DEBUG),
                ),
        )
        .layer(axum::middleware::from_fn(log_rate_limited))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(build_cors_layer(frontend_url.as_deref()))
        .with_state(state);

    // Read PORT from env (set in .env or environment); default 3000.
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse()
        .expect("PORT must be a valid number");
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // into_make_service_with_connect_info is required for PeerIpKeyExtractor to work.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn log_rate_limited(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let peer = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.to_string())
        .unwrap_or_else(|| "unknown".into());

    let response = next.run(req).await;

    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        tracing::warn!(
            peer = %peer,
            method = %method,
            uri = %uri,
            "rate limited (429)"
        );
    }

    response
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}

/// Build CORS layer with frontend origin restriction.
/// If FRONTEND_URL is set, restricts to that origin.
/// Otherwise, allows any origin (development mode).
fn build_cors_layer(frontend_url: Option<&str>) -> CorsLayer {
    let mut cors = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE])
        .expose_headers([
            axum::http::header::ETAG,
            axum::http::header::CACHE_CONTROL,
        ])
        .max_age(Duration::from_secs(3600));

    if let Some(origin) = frontend_url {
        // Production: restrict to specific frontend origin
        // Note: We don't allow credentials for CORS - cookies are SameSite=Strict
        match origin.parse::<axum::http::HeaderValue>() {
            Ok(parsed_origin) => {
                cors = cors.allow_origin(parsed_origin);
                tracing::info!("CORS restricted to origin: {}", origin);
            }
            Err(e) => {
                tracing::warn!("Invalid FRONTEND_URL '{}': {}. Allowing any origin.", origin, e);
                cors = cors.allow_origin(Any);
            }
        }
    } else {
        // Development: allow any origin
        cors = cors.allow_origin(Any);
        tracing::warn!("FRONTEND_URL not set - CORS allowing any origin (development mode)");
    }
    
    cors
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_app(cors: CorsLayer) -> Router {
        Router::new()
            .route("/ping", get(|| async { "pong" }))
            .layer(cors)
    }

    #[tokio::test]
    async fn cors_any_origin_when_no_frontend_url() {
        let cors = build_cors_layer(None);
        let app = test_app(cors);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ping")
                    .header("Origin", "http://evil.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let origin = resp.headers().get("access-control-allow-origin").unwrap();
        assert_eq!(origin, "*");
    }

    #[tokio::test]
    async fn cors_restricted_to_frontend_url() {
        let cors = build_cors_layer(Some("http://localhost:3001"));
        let app = test_app(cors);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ping")
                    .header("Origin", "http://localhost:3001")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let origin = resp.headers().get("access-control-allow-origin").unwrap();
        assert_eq!(origin, "http://localhost:3001");
    }

    #[tokio::test]
    async fn cors_falls_back_on_invalid_frontend_url() {
        // A header value can't contain newlines — this should trigger the fallback
        let cors = build_cors_layer(Some("http://bad\norigin"));
        let app = test_app(cors);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ping")
                    .header("Origin", "http://anything.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let origin = resp.headers().get("access-control-allow-origin").unwrap();
        assert_eq!(origin, "*");
    }
}
