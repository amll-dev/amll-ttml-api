pub use core::db::setup::init_db;
use std::time::Duration;

use axum::{
    Router,
    middleware::{
        from_fn,
        from_fn_with_state,
    },
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

pub use crate::{
    analytics::{
        Analytics,
        AnalyticsConfig,
        AnalyticsWriter,
    },
    services::AppState,
};
use crate::{
    core::error::AppError,
    utils::cors::create_cors_layer,
};

mod analytics;
mod api;
mod core;
mod services;
mod utils;

#[cfg(test)]
mod wire_format_tests;

/// 同一套 v1 路由挂载的两个前缀
///
/// 请求统计按它区分客户端走的是哪个前缀，以后决定是否下线某个前缀时要用
pub(crate) const API_PREFIXES: [&str; 2] = ["/v1", "/api/v1"];

pub fn create_app(state: AppState) -> Router {
    let v1_routes = Router::new()
        .route("/status", get(api::status::handler::handle_status))
        .route("/version", get(api::status::handler::handle_status))
        .route("/lyrics/get", get(api::get::handler::handle_get))
        .route("/lyrics/search", get(api::search::handler::handle_search))
        .route("/lyrics/list", get(api::list::handler::handle_list))
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

    let mut router = Router::new();
    for prefix in API_PREFIXES {
        router = router.nest(prefix, v1_routes.clone());
    }

    let router = router
        .fallback(|| async { AppError::NotFound })
        .layer(from_fn(api::shared::etag::apply))
        .layer(NewSentryLayer::new_from_top())
        .layer(SentryHttpLayer::new().enable_transaction())
        .layer(create_cors_layer())
        .layer(trace_layer);

    // 请求统计挂在最外层，未启用时不挂载
    let router = match state.analytics.clone() {
        Some(handle) => router.layer(from_fn_with_state(handle, analytics::record)),
        None => router,
    };

    router.with_state(state)
}
