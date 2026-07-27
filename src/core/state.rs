use std::{
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;
use moka::future::Cache;
use reqwest::Client;
use tracing::{
    error,
    info,
};

use crate::{
    core::{
        error::AppError,
        models::LyricIndexDB,
    },
    services::github_fetcher::{
        fetch_and_parse_db,
        fetch_raw_lyric,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<ArcSwap<LyricIndexDB>>,
    pub ttml_cache: Cache<String, String>,
    pub http_client: Client,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
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
            ttml_cache,
            http_client,
        }
    }

    pub async fn update_db(&self) -> Result<(), AppError> {
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

    pub async fn fetch_lyric_ttml(&self, filename: &str) -> Result<String, AppError> {
        if let Some(cached) = self.ttml_cache.get(filename).await {
            return Ok(cached);
        }

        let text = fetch_raw_lyric(&self.http_client, filename).await?;
        self.ttml_cache
            .insert(filename.to_string(), text.clone())
            .await;
        Ok(text)
    }
}
