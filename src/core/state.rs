use std::{
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;
use moka::future::Cache;
use reqwest::Client;
use sea_orm::{
    ColumnTrait,
    ConnectionTrait,
    DatabaseBackend,
    DatabaseConnection,
    EntityTrait,
    QueryFilter,
    Statement,
};
use tracing::{
    error,
    info,
};

use crate::{
    core::{
        db::entity,
        error::AppError,
        matcher::convert_tw2s,
        models::{
            LyricHit,
            LyricIndexDB,
            LyricMatchField,
        },
    },
    services::{
        LyricStore,
        github_fetcher::{
            fetch_and_parse_db,
            fetch_raw_lyric,
        },
        sync_service::SyncService,
    },
    utils::{
        highlight::extract_lyric_context,
        ttml::{
            TTMLFormatResult,
            parse_and_format_ttml,
        },
    },
};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<ArcSwap<LyricIndexDB>>,
    pub db_conn: DatabaseConnection,
    pub sync_lock: Arc<tokio::sync::Mutex<()>>,
    pub ttml_cache: Cache<String, String>,
    pub formatted_lyric_cache: Cache<String, TTMLFormatResult>,
    pub http_client: Client,
    pub start_time: std::time::Instant,
}

impl AppState {
    #[must_use]
    pub fn new(db_conn: DatabaseConnection) -> Self {
        let http_client = Client::builder()
            .user_agent("amll-ttml-api/0.1")
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());

        let ttml_cache = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(Duration::from_hours(168))
            .build();

        let formatted_lyric_cache = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(Duration::from_hours(168))
            .build();

        Self {
            db: Arc::new(ArcSwap::from_pointee(LyricIndexDB::default())),
            db_conn,
            sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            ttml_cache,
            formatted_lyric_cache,
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
        let syncer = SyncService::new(self.db_conn.clone(), self.http_client.clone());
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
    }

    pub async fn fetch_lyric_ttml(&self, filename: &str) -> Result<String, AppError> {
        if let Some(cached) = self.ttml_cache.get(filename).await {
            return Ok(cached);
        }

        if let Ok(Some(row)) = entity::Entity::find()
            .filter(entity::Column::Filename.eq(filename))
            .one(&self.db_conn)
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

    pub async fn fetch_parsed_lyric(&self, filename: &str) -> Result<TTMLFormatResult, AppError> {
        if let Some(cached) = self.formatted_lyric_cache.get(filename).await {
            return Ok(cached);
        }

        let ttml = self.fetch_lyric_ttml(filename).await?;
        let formatted = parse_and_format_ttml(&ttml);

        self.formatted_lyric_cache
            .insert(filename.to_string(), formatted.clone())
            .await;

        Ok(formatted)
    }

    pub async fn search_lyrics_fts(
        &self,
        keyword: &str,
        limit: u64,
    ) -> Result<Vec<LyricHit>, AppError> {
        if keyword.trim().is_empty() {
            return Ok(Vec::new());
        }

        let normalized = convert_tw2s(keyword).to_lowercase();
        let safe_keyword = normalized.replace('"', "\"\"");
        let match_expr = format!("\"{safe_keyword}\"");

        let sql = r"
            SELECT 
                rowid AS id, 
                bm25(lyrics_fts, 0.0, 1.0, 0.25) AS rank, 
                lyric_text,
                bg_vocal_text
            FROM lyrics_fts 
            WHERE lyrics_fts MATCH $1
            ORDER BY rank ASC 
            LIMIT $2;
        ";

        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            sql,
            vec![match_expr.into(), limit.into()],
        );

        let rows = self
            .db_conn
            .query_all_raw(stmt)
            .await
            .map_err(|e| AppError::InternalServerError(format!("FTS Query Error: {e}")))?;

        let mut hits = Vec::with_capacity(rows.len());
        for row in rows {
            let id: i64 = match row.try_get("", "id") {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!("Failed to read 'id' column from FTS result: {e}");
                    continue;
                }
            };

            let rank: f64 = match row.try_get("", "rank") {
                Ok(rank) => rank,
                Err(e) => {
                    tracing::warn!("Failed to read 'rank' column from FTS result: {e}");
                    continue;
                }
            };

            let lyric_text: String = row.try_get("", "lyric_text").unwrap_or_default();
            let bg_vocal_text: String = row.try_get("", "bg_vocal_text").unwrap_or_default();

            let (field, snippet) = extract_lyric_context(&lyric_text, &normalized)
                .map(|s| (LyricMatchField::MainLyric, Some(s)))
                .or_else(|| {
                    extract_lyric_context(&bg_vocal_text, &normalized)
                        .map(|s| (LyricMatchField::BackgroundVocal, Some(s)))
                })
                .unwrap_or((LyricMatchField::MainLyric, None));

            hits.push(LyricHit {
                id: id.cast_unsigned(),
                rank,
                field,
                snippet,
            });
        }

        Ok(hits)
    }
}

#[allow(clippy::unused_async_trait_impl)]
impl LyricStore for AppState {
    async fn fetch_lyric_ttml(&self, filename: &str) -> Result<String, AppError> {
        self.fetch_lyric_ttml(filename).await
    }

    async fn fetch_parsed_lyric(&self, filename: &str) -> Result<TTMLFormatResult, AppError> {
        self.fetch_parsed_lyric(filename).await
    }

    async fn search_lyrics_fts(
        &self,
        keyword: &str,
        limit: u64,
    ) -> Result<Vec<LyricHit>, AppError> {
        self.search_lyrics_fts(keyword, limit).await
    }

    async fn load_index(&self) -> Arc<LyricIndexDB> {
        self.db.load_full()
    }
}
