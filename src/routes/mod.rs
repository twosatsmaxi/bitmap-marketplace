use axum::Router;
use crate::AppState;

mod activity;
mod collections;
mod inscriptions;
mod listings;
mod offers;
mod orders;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/activity", activity::router())
        .nest("/collections", collections::router())
        .nest("/inscriptions", inscriptions::router())
        .nest("/listings", listings::router())
        .nest("/offers", offers::router())
        .nest("/orders", orders::router())
}
