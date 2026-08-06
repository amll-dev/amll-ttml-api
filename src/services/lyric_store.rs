use std::sync::Arc;

use crate::{
    core::{
        error::AppError,
        models::{
            LyricHit,
            LyricIndexDB,
        },
    },
    utils::ttml::TTMLFormatResult,
};

pub trait LyricStore: Send + Sync {
    async fn fetch_lyric_ttml(&self, filename: &str) -> Result<String, AppError>;
    async fn fetch_parsed_lyric(&self, filename: &str) -> Result<TTMLFormatResult, AppError>;
    async fn search_lyrics_fts(&self, keyword: &str, limit: u64)
    -> Result<Vec<LyricHit>, AppError>;
    async fn load_index(&self) -> Arc<LyricIndexDB>;
}
