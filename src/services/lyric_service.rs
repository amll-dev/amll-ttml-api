use worker::RouteContext;

use crate::{
    api::shared::dto::{
        ApiResponse,
        SearchData,
        SongItem,
        map_song_to_item,
    },
    core::{
        error::AppError,
        models::{
            IdQuery,
            SearchQuery,
        },
    },
    services::github_fetcher::{
        acquire_db_read_lock,
        fetch_lyric_ttml,
    },
};

pub struct LyricService;

impl LyricService {
    pub async fn search_lyric(
        ctx: &RouteContext<worker::Context>,
        query: SearchQuery,
        limit: usize,
    ) -> Result<ApiResponse<SearchData>, AppError> {
        let state = acquire_db_read_lock(ctx).await?;
        let matched_songs = state.db.search_by_fields(&query);
        let songs_cloned: Vec<_> = matched_songs.into_iter().take(limit).cloned().collect();

        drop(state);

        let items: Vec<SongItem> = songs_cloned
            .iter()
            .map(|entry| map_song_to_item(entry, None, None))
            .collect();

        Ok(ApiResponse {
            status: 200,
            data: SearchData { items },
        })
    }

    pub async fn get_lyric(
        ctx: &RouteContext<worker::Context>,
        query: IdQuery,
        format: String,
    ) -> Result<ApiResponse<SongItem>, AppError> {
        let state = acquire_db_read_lock(ctx).await?;
        let matched_indices = state.db.find_by_ids(&query);

        if matched_indices.is_empty() {
            return Err(AppError::LyricNotFound);
        }

        let mut candidates: Vec<_> = matched_indices
            .into_iter()
            .map(|idx| &state.db.entries[idx])
            .collect();
        candidates.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

        let latest_song_cloned = candidates[0].clone();
        drop(state);

        let ttml_text = fetch_lyric_ttml(latest_song_cloned.filename.as_str()).await?;

        let item = map_song_to_item(&latest_song_cloned, Some(ttml_text), Some(format));

        Ok(ApiResponse {
            status: 200,
            data: item,
        })
    }
}
