use std::{
    env,
    time::Duration,
};

use amll_ttml_api::{
    Analytics,
    AnalyticsConfig,
    AnalyticsWriter,
    AppState,
    create_app,
    init_db,
};
use anyhow::Result;
use tokio::net::TcpListener;
use tracing::{
    error,
    info,
};
use tracing_subscriber::{
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let sentry_dsn = env::var("SENTRY_DSN").ok();
    // 逐请求数据由请求统计负责，Sentry 默认只上报错误；排查性能问题时再临时调高采样率
    let traces_sample_rate = env::var("SENTRY_TRACES_SAMPLE_RATE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|&rate| (0.0..=1.0).contains(&rate))
        .unwrap_or(0.0);

    let mut options = sentry::ClientOptions::default();
    options.release = sentry::release_name!();
    options = options.traces_sample_rate(traces_sample_rate);
    let _sentry_guard = sentry::init((sentry_dsn, options));

    let sentry_layer = sentry_tracing::layer().event_filter(|md| match *md.level() {
        tracing::Level::ERROR | tracing::Level::WARN => sentry_tracing::EventFilter::Event,
        tracing::Level::INFO => sentry_tracing::EventFilter::Breadcrumb,
        _ => sentry_tracing::EventFilter::Ignore,
    });

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "amll_ttml_api=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(sentry_layer)
        .init();

    let db_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://data/amll_lyrics.db?mode=rwc".to_string());

    let db_conn = init_db(&db_url).await.map_err(|e| {
        error!("Startup error: Failed to initialize SQLite database at `{db_url}`: {e:?}");
        e
    })?;

    info!("Initialized SQLite database connection pool at {db_url}");

    let port = env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3000);

    let (state, analytics_writer) = start_analytics(AppState::new(db_conn), port).await;

    // 启动时从本地数据库建立内存索引，搜索立即可用
    // 如果本地没有数据库，需要等待 `LyricSyncer.sync` 的第一次同步
    if let Err(e) = state.store.rebuild_index().await {
        error!("Startup index rebuild failed, serving with empty index until first sync: {e:?}");
    }
    info!(
        "In-memory index ready with {} entries",
        state.store.lyric_count()
    );

    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Err(e) = state_clone.syncer.sync(false).await {
            error!("Initial DB fetch/sync failed: {e:?}");
        }
    });

    let state_periodic = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_hours(24));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            info!("Periodic DB update triggered (daily fallback)");
            if let Err(e) = state_periodic.syncer.sync(false).await {
                error!("Periodic DB update failed: {e:?}");
            }
        }
    });

    let app = create_app(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await.map_err(|e| {
        error!("Startup error: Failed to bind to `{addr}`: {e:?}");
        e
    })?;

    info!("AMLL TTML API Server listening on http://{addr}");

    let shutdown_signal = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install signal handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            () = ctrl_c => {},
            () = terminate => {},
        }
    };

    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await;

    // 在途请求都已结束，它们的统计记录都在 channel 里，此时刷盘不会漏
    if let Some(writer) = analytics_writer {
        writer.shutdown().await;
    }

    served.map_err(|e| {
        error!("Server error: {e:?}");
        e
    })?;

    Ok(())
}

/// 按环境变量启用请求统计
///
/// 统计出任何问题都不能影响主服务启动，失败只关闭统计
async fn start_analytics(state: AppState, port: u16) -> (AppState, Option<AnalyticsWriter>) {
    let Some(config) = AnalyticsConfig::from_env(port) else {
        info!("Request analytics disabled (ANALYTICS_DIR not set)");
        return (state, None);
    };

    let dir = config.dir.clone();
    match Analytics::start(config).await {
        Ok((analytics, writer)) => {
            info!("Request analytics enabled, buffering to {}", dir.display());
            (state.with_analytics(analytics), Some(writer))
        }
        Err(e) => {
            error!("Failed to start request analytics, continuing without it: {e:?}");
            (state, None)
        }
    }
}
