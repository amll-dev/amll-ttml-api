//! 生产环境的歌词存储库，负责获取和更新歌词数据

use std::{
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;
use moka::future::Cache;
use reqwest::Client;
use sea_orm::DatabaseConnection;

use crate::{
    core::{
        db::queries::{
            find_raw_ttml,
            search_fts,
        },
        error::AppError,
        models::{
            LyricHit,
            LyricIndexDB,
        },
    },
    services::{
        LyricStore,
        github_fetcher::fetch_raw_lyric,
    },
    utils::ttml::{
        TTMLFormatResult,
        parse_and_format_ttml,
    },
};

#[derive(Clone)]
pub struct DbLyricStore {
    pub index: Arc<ArcSwap<LyricIndexDB>>,
    pub conn: DatabaseConnection,
    pub client: Client,
    pub ttml_cache: Cache<String, String>,
    pub formatted_lyric_cache: Cache<String, TTMLFormatResult>,
}

impl DbLyricStore {
    /// 构造一个从空索引启动的存储
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        let client = Client::builder()
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
            index: Arc::new(ArcSwap::from_pointee(LyricIndexDB::default())),
            conn,
            client,
            ttml_cache,
            formatted_lyric_cache,
        }
    }

    /// 当前索引条目数
    #[must_use]
    pub fn lyric_count(&self) -> usize {
        self.index.load().entries.len()
    }

    /// 原子写入新索引
    pub fn swap_index(&self, new_index: LyricIndexDB) {
        self.index.store(Arc::new(new_index));
    }

    /// 使所有的缓存失效
    pub fn invalidate_caches(&self) {
        self.ttml_cache.invalidate_all();
        self.formatted_lyric_cache.invalidate_all();
    }

    /// 根据文件名获取 TTML 文件内容
    ///
    /// 获取顺序是 缓存 -> 本地数据库 -> GitHub 获取兜底
    pub async fn fetch_lyric_ttml(&self, filename: &str) -> Result<String, AppError> {
        if let Some(cached) = self.ttml_cache.get(filename).await {
            return Ok(cached);
        }

        if let Some(ttml) = find_raw_ttml(&self.conn, filename).await {
            self.ttml_cache
                .insert(filename.to_string(), ttml.clone())
                .await;
            return Ok(ttml);
        }

        let text = fetch_raw_lyric(&self.client, filename).await?;
        self.ttml_cache
            .insert(filename.to_string(), text.clone())
            .await;
        Ok(text)
    }

    /// 根据文件名获取解析后的 [`TTMLFormatResult`]，包含纯文本歌词、LRC 歌词和时长
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

    /// FTS 全文检索
    pub async fn search_lyrics_fts(
        &self,
        keyword: &str,
        limit: u64,
    ) -> Result<Vec<LyricHit>, AppError> {
        search_fts(&self.conn, keyword, limit).await
    }
}

#[allow(clippy::unused_async_trait_impl)]
impl LyricStore for DbLyricStore {
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
        self.index.load_full()
    }
}
