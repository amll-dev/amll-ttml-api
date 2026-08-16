use std::{
    env,
    time::Duration,
};

use amll_ttml_api::{
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
    let traces_sample_rate = env::var("SENTRY_TRACES_SAMPLE_RATE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|&rate| (0.0..=1.0).contains(&rate))
        .unwrap_or(1.0);

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

    let state = AppState::new(db_conn);

    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Err(e) = state_clone.syncer.sync().await {
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
            if let Err(e) = state_periodic.syncer.sync().await {
                error!("Periodic DB update failed: {e:?}");
            }
        }
    });

    let app = create_app(state);

    let port = env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3000);

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

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await
        .map_err(|e| {
            error!("Server error: {e:?}");
            e
        })?;

    Ok(())
}
