use worker::RouteContext;

use crate::{
    api::{
        lrclib::dto::{
            LrclibSongItem,
            map_to_lrclib_item,
        },
        shared::dto::{
            ApiResponse,
            SearchData,
            SongItem,
            map_song_to_item,
        },
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
    utils::matcher::{
        MatchType,
        PreparedQuery,
        score_entry,
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
        let items: Vec<SongItem> = matched_songs
            .into_iter()
            .take(limit)
            .map(|entry| map_song_to_item(entry, None, None))
            .collect();

        drop(state);
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

    pub async fn lrclib_search(
        ctx: &RouteContext<worker::Context>,
        query: SearchQuery,
        limit: usize,
    ) -> Result<Vec<LrclibSongItem>, AppError> {
        let state = acquire_db_read_lock(ctx).await?;

        let matched_songs = state.db.search_by_fields(&query);
        let items: Vec<LrclibSongItem> = matched_songs
            .into_iter()
            .take(limit)
            .map(|entry| map_to_lrclib_item(entry, None))
            .collect();

        drop(state);
        Ok(items)
    }

    pub async fn lrclib_get_by_fields(
        ctx: &RouteContext<worker::Context>,
        query: SearchQuery,
    ) -> Result<LrclibSongItem, AppError> {
        let state = acquire_db_read_lock(ctx).await?;

        let prepared = PreparedQuery::from_search_query(&query);
        let matched_songs = state.db.search_by_fields(&query);

        if matched_songs.is_empty() {
            return Err(AppError::LyricNotFound);
        }

        let best_match = matched_songs[0];
        let score = score_entry(&prepared, best_match);

        if score < MatchType::Medium {
            return Err(AppError::LyricNotFound);
        }

        let latest_song_cloned = best_match.clone();
        drop(state);

        let ttml_text = fetch_lyric_ttml(latest_song_cloned.filename.as_str()).await?;
        let item = map_to_lrclib_item(&latest_song_cloned, Some(ttml_text));
        Ok(item)
    }

    pub async fn lrclib_get_by_id(
        ctx: &RouteContext<worker::Context>,
        id: u64,
    ) -> Result<LrclibSongItem, AppError> {
        let state = acquire_db_read_lock(ctx).await?;

        let idx = state
            .db
            .id_idx
            .get(&id)
            .copied()
            .ok_or(AppError::LyricNotFound)?;
        let song_cloned = state.db.entries[idx].clone();
        drop(state);

        let ttml_text = fetch_lyric_ttml(song_cloned.filename.as_str()).await?;
        let item = map_to_lrclib_item(&song_cloned, Some(ttml_text));
        Ok(item)
    }
}
