use anyhow::Result;
use axum::Router;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod db;
mod errors;
mod models;
mod routes;
mod services;
pub mod ws;

use crate::db::Database;

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub ws_broadcaster: std::sync::Arc<ws::WsBroadcaster>,
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

    let db = Database::new().await?;
    db.run_migrations().await?;

    let ws_broadcaster = std::sync::Arc::new(ws::WsBroadcaster::new());

    let state = AppState { db, ws_broadcaster: ws_broadcaster.clone() };

    let app = Router::new()
        .nest("/api", routes::router())
        .merge(ws::router(ws_broadcaster))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
