use anyhow::Result;
use axum::Router;
use bitcoin::Network;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tower::limit::ConcurrencyLimitLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::GovernorLayer;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
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

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub ws_broadcaster: Arc<ws::WsBroadcaster>,
    pub marketplace_keypair: Arc<MarketplaceKeypair>,
    pub http_client: reqwest::Client,
    pub ord_client: OrdClient,
    pub render_api_base: String,
    pub network: Network,
    pub marketplace_fee_address: Option<String>,
    pub marketplace_fee_bps: u64,
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

    let render_api_base =
        std::env::var("RENDER_API_BASE").unwrap_or_else(|_| "http://r2d2.local:3020".to_string());

    let marketplace_fee_address = std::env::var("MARKETPLACE_FEE_ADDRESS").ok();
    let marketplace_fee_bps: u64 = std::env::var("MARKETPLACE_FEE_BPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let state = AppState {
        db,
        ws_broadcaster: ws_broadcaster.clone(),
        marketplace_keypair,
        http_client: reqwest::Client::new(),
        ord_client: OrdClient::new(),
        render_api_base,
        network,
        marketplace_fee_address,
        marketplace_fee_bps,
    };

    // Per-IP rate limiting config.
    // Uses PeerIpKeyExtractor (TCP peer address) — correct for direct deployments with no
    // reverse proxy. If deployed behind a load balancer/proxy later, switch to
    // SmartIpKeyExtractor and configure trusted proxy allowlisting to prevent header spoofing.
    // 30-burst / 10s-refill: generous for legitimate marketplace users, strict for scrapers.
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(10)
            .burst_size(30)
            .finish()
            .unwrap(),
    );

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
        .nest("/api", api_router)
        .merge(ws_router)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive()) // public marketplace; keep permissive
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

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");
    tracing::info!("shutdown signal received");
}
