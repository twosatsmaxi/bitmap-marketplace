use axum::Router;
use crate::AppState;

mod collections;
mod inscriptions;
mod listings;
mod orders;
mod activity;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/collections", collections::router())
        .nest("/inscriptions", inscriptions::router())
        .nest("/listings", listings::router())
        .nest("/orders", orders::router())
        .nest("/activity", activity::router())
}
