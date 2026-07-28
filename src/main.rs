use std::{
    env,
    time::Duration,
};

use amll_ttml_api::{
    AppState,
    create_app,
    init_db,
};
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
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "amll_ttml_api=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://data/amll_lyrics.db?mode=rwc".to_string());

    let db_conn = match init_db(&db_url).await {
        Ok(conn) => {
            info!("Initialized SQLite database connection pool at {db_url}");
            Some(conn)
        }
        Err(e) => {
            error!("Failed to initialize database: {e:?}, falling back to memory mode");
            None
        }
    };

    let state = AppState::new(db_conn);

    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Err(e) = state_clone.update_db().await {
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
            if let Err(e) = state_periodic.update_db().await {
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
    let listener = TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind to {addr}: {e}"));

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
        .unwrap_or_else(|e| panic!("Server error: {e}"));
}
