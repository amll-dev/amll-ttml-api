use std::{
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;
use moka::future::Cache;
use reqwest::Client;
use sea_orm::{
    ColumnTrait,
    DatabaseConnection,
    EntityTrait,
    QueryFilter,
};
use tracing::{
    error,
    info,
};

use crate::{
    core::{
        db::entity,
        error::AppError,
        models::LyricIndexDB,
    },
    services::{
        github_fetcher::{
            fetch_and_parse_db,
            fetch_raw_lyric,
        },
        sync_service::SyncService,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<ArcSwap<LyricIndexDB>>,
    pub db_conn: Option<DatabaseConnection>,
    pub sync_lock: Arc<tokio::sync::Mutex<()>>,
    pub ttml_cache: Cache<String, String>,
    pub http_client: Client,
    pub start_time: std::time::Instant,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(None)
    }
}

impl AppState {
    #[must_use]
    pub fn new(db_conn: Option<DatabaseConnection>) -> Self {
        let http_client = Client::builder()
            .user_agent("amll-ttml-api/0.1")
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());

        let ttml_cache = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(Duration::from_hours(168))
            .build();

        Self {
            db: Arc::new(ArcSwap::from_pointee(LyricIndexDB::default())),
            db_conn,
            sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            ttml_cache,
            http_client,
            start_time: std::time::Instant::now(),
        }
    }

    #[must_use]
    pub fn lyric_count(&self) -> usize {
        self.db.load().entries.len()
    }

    pub async fn update_db(&self) -> Result<(), AppError> {
        let Ok(_guard) = self.sync_lock.try_lock() else {
            info!("Database sync is already in progress, skipping duplicate request.");
            return Ok(());
        };

        info!("Running database sync service...");
        if let Some(ref db_conn) = self.db_conn {
            let syncer = SyncService::new(db_conn.clone(), self.http_client.clone());
            match syncer.sync().await {
                Ok(res) => {
                    info!("Sync completed with status: {:?}", res.status);
                    if let Ok(new_db) = fetch_and_parse_db(&self.http_client).await {
                        self.db.store(Arc::new(new_db));
                    }
                    Ok(())
                }
                Err(e) => {
                    error!("Sync failed: {e:?}");
                    Err(AppError::UpstreamError(e.to_string()))
                }
            }
        } else {
            info!("Fetching latest lyric index from GitHub...");
            match fetch_and_parse_db(&self.http_client).await {
                Ok(new_db) => {
                    let count = new_db.entries.len();
                    self.db.store(Arc::new(new_db));
                    info!("Successfully updated lyric index DB with {count} entries");
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to fetch lyric index: {e:?}");
                    Err(e)
                }
            }
        }
    }

    pub async fn fetch_lyric_ttml(&self, filename: &str) -> Result<String, AppError> {
        if let Some(cached) = self.ttml_cache.get(filename).await {
            return Ok(cached);
        }

        if let Some(ref db_conn) = self.db_conn
            && let Ok(Some(row)) = entity::Entity::find()
                .filter(entity::Column::Filename.eq(filename))
                .one(db_conn)
                .await
            && !row.raw_ttml.is_empty()
        {
            self.ttml_cache
                .insert(filename.to_string(), row.raw_ttml.clone())
                .await;
            return Ok(row.raw_ttml);
        }

        let text = fetch_raw_lyric(&self.http_client, filename).await?;
        self.ttml_cache
            .insert(filename.to_string(), text.clone())
            .await;
        Ok(text)
    }
}
