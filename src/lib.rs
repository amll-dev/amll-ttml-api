pub use core::state::AppState;

use axum::{
    Router,
    routing::get,
};
use tower_http::trace::TraceLayer;

use crate::{
    core::error::AppError,
    utils::cors::create_cors_layer,
};

mod api;
mod core;
mod services;
mod utils;

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/lyrics/get", get(api::get::handler::handle_get))
        .route(
            "/api/v1/lyrics/search",
            get(api::search::handler::handle_search),
        )
        .route(
            "/api/v1/lrclib/search",
            get(api::lrclib::handler::handle_search),
        )
        .route("/api/v1/lrclib/get", get(api::lrclib::handler::handle_get))
        .route(
            "/api/v1/lrclib/get/{id}",
            get(api::lrclib::handler::handle_get_by_id),
        )
        .fallback(|| async { AppError::NotFound })
        .layer(create_cors_layer())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
