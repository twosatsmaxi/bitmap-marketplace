use crate::AppState;
use axum::Router;

mod activity;
mod bitmaps;
mod collections;
mod explore;
mod inscriptions;
mod listings;
mod offers;
mod orders;

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
}
