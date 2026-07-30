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
    let v1_routes = Router::new()
        .route("/status", get(api::status::handler::handle_status))
        .route("/version", get(api::status::handler::handle_status))
        .route("/lyrics/get", get(api::get::handler::handle_get))
        .route("/lyrics/search", get(api::search::handler::handle_search))
        .route("/lrclib/search", get(api::lrclib::handler::handle_search))
        .route("/lrclib/get", get(api::lrclib::handler::handle_get))
        .route(
            "/lrclib/get/{id}",
            get(api::lrclib::handler::handle_get_by_id),
        )
        .route(
            "/webhook/sync",
            post(api::webhook::handler::handle_webhook_sync),
        );

    Router::new()
        .nest("/v1", v1_routes.clone())
        .nest("/api/v1", v1_routes)
        .fallback(|| async { AppError::NotFound })
        .layer(NewSentryLayer::new_from_top())
        .layer(SentryHttpLayer::new().enable_transaction())
        .layer(create_cors_layer())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
