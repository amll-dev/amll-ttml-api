use worker::RouteContext;

use crate::{
    api::shared::dto::{
        ApiResponseEntry,
        map_song_to_dto,
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
    ) -> Result<Vec<ApiResponseEntry>, AppError> {
        let state = acquire_db_read_lock(ctx).await?;
        let matched_songs = state.db.search_by_fields(&query);
        let songs_cloned: Vec<_> = matched_songs.into_iter().take(limit).cloned().collect();

        drop(state);

        let results_dto: Vec<ApiResponseEntry> = songs_cloned
            .iter()
            .map(|entry| map_song_to_dto(entry, None))
            .collect();

        Ok(results_dto)
    }

    pub async fn get_lyric(
        ctx: &RouteContext<worker::Context>,
        query: IdQuery,
    ) -> Result<Vec<ApiResponseEntry>, AppError> {
        let state = acquire_db_read_lock(ctx).await?;
        let matched_indices = state.db.find_by_ids(&query);

        if matched_indices.is_empty() {
            return Ok(vec![]);
        }

        let mut candidates: Vec<_> = matched_indices
            .into_iter()
            .map(|idx| &state.db.entries[idx])
            .collect();
        candidates.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

        let latest_song_cloned = candidates[0].clone();
        drop(state);

        let ttml_text = fetch_lyric_ttml(latest_song_cloned.filename.as_str()).await?;

        let api_res = map_song_to_dto(&latest_song_cloned, Some(ttml_text));

        Ok(vec![api_res])
    }
}
