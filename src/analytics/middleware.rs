//! 逐请求采集的 axum 中间件
//!
//! 挂在路由最外层（比 CORS、Trace 都靠外），CORS 预检、未匹配路由的 404 fallback 与
//! `ETag` 降级后的 304 都能被看到；耗时也因此覆盖了整条中间件链

use std::{
    sync::Arc,
    time::Instant,
};

use axum::{
    extract::{
        Request,
        State,
    },
    middleware::Next,
    response::Response,
};

use super::{
    Analytics,
    annotation::Annotator,
    record::PendingRecord,
};

pub async fn record(State(analytics): State<Analytics>, mut req: Request, next: Next) -> Response {
    let started = Instant::now();
    let pending = PendingRecord::capture(&req, analytics.ip_hasher.as_deref());

    let annotator = Annotator::enabled();
    req.extensions_mut().insert(annotator.clone());

    let response = next.run(req).await;

    analytics.submit(pending.finish(
        &response,
        started.elapsed(),
        annotator.take(),
        Arc::clone(&analytics.instance),
    ));

    response
}
