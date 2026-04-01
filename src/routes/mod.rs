use crate::AppState;
use axum::Router;

mod activity;
pub mod auth;
mod bitmaps;
mod collections;
mod explore;
mod inscriptions;
mod listings;
mod offers;
mod orders;
mod portfolio;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/activity", activity::router())
        .nest("/bitmap", bitmaps::router())
        .nest("/collections", collections::router())
        .nest("/explore", explore::router())
        .nest("/inscriptions", inscriptions::router())
        .nest("/listings", listings::router())
        .nest("/offers", offers::router())
        .nest("/orders", orders::router())
        .nest("/portfolio", portfolio::router())
    // NOTE: auth is mounted separately in main.rs with stricter rate limiting
}

pub fn auth_router() -> Router<AppState> {
    auth::router()
}
