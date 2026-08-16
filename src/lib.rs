pub use core::db::setup::init_db;
use std::time::Duration;

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
use tower_http::trace::{
    DefaultMakeSpan,
    TraceLayer,
};
use tracing::{
    Level,
    info,
};

pub use crate::services::AppState;
use crate::{
    core::error::AppError,
    utils::cors::create_cors_layer,
};

mod api;
mod core;
mod services;
mod utils;

#[cfg(test)]
mod wire_format_tests;

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

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
        .on_response(
            |response: &axum::http::Response<_>, latency: Duration, _span: &tracing::Span| {
                info!(
                    status = response.status().as_u16(),
                    latency_ms = %format_args!("{latency:.2?}"),
                    "HTTP request completed"
                );
            },
        );

    Router::new()
        .nest("/v1", v1_routes.clone())
        .nest("/api/v1", v1_routes)
        .fallback(|| async { AppError::NotFound })
        .layer(NewSentryLayer::new_from_top())
        .layer(SentryHttpLayer::new().enable_transaction())
        .layer(create_cors_layer())
        .layer(trace_layer)
        .with_state(state)
}
