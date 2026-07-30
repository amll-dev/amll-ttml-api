pub use core::{
    db::setup::init_db,
    state::AppState,
};

use axum::{
    Router,
    routing::{
        get,
        post,
    },
};
use sentry_tower::{
    NewSentryLayer,
    SentryHttpLayer,
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
        .route("/api/v1/status", get(api::status::handler::handle_status))
        .route("/api/v1/version", get(api::status::handler::handle_status))
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
        .route(
            "/api/v1/webhook/sync",
            post(api::webhook::handler::handle_webhook_sync),
        )
        .fallback(|| async { AppError::NotFound })
        .layer(NewSentryLayer::new_from_top())
        .layer(SentryHttpLayer::new().enable_transaction())
        .layer(create_cors_layer())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
