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
        state::AppState,
    },
    utils::matcher::{
        MatchType,
        PreparedQuery,
        score_entry,
    },
};

pub struct LyricService;

impl LyricService {
    pub fn search_lyric(
        state: &AppState,
        query: &SearchQuery,
        limit: usize,
    ) -> ApiResponse<SearchData> {
        let db = state.db.load();

        let matched_songs = db.search_by_fields(query);
        let items: Vec<SongItem> = matched_songs
            .into_iter()
            .take(limit)
            .map(|entry| map_song_to_item(entry, None, None))
            .collect();

        drop(db);
        ApiResponse {
            status: 200,
            data: SearchData { items },
        }
    }

    pub async fn get_lyric(
        state: &AppState,
        query: IdQuery,
        format: String,
    ) -> Result<ApiResponse<SongItem>, AppError> {
        let db = state.db.load();
        let matched_indices = db.find_by_ids(&query);

        if matched_indices.is_empty() {
            return Err(AppError::LyricNotFound);
        }

        let mut candidates: Vec<_> = matched_indices
            .into_iter()
            .map(|idx| &db.entries[idx])
            .collect();
        candidates.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

        let latest_song_cloned = candidates[0].clone();
        drop(db);

        let ttml_text = state
            .fetch_lyric_ttml(latest_song_cloned.filename.as_str())
            .await?;

        let item = map_song_to_item(&latest_song_cloned, Some(ttml_text), Some(format));

        Ok(ApiResponse {
            status: 200,
            data: item,
        })
    }

    pub fn lrclib_search(
        state: &AppState,
        query: &SearchQuery,
        limit: usize,
    ) -> Vec<LrclibSongItem> {
        let db = state.db.load();

        let matched_songs = db.search_by_fields(query);
        let items: Vec<LrclibSongItem> = matched_songs
            .into_iter()
            .take(limit)
            .map(|entry| map_to_lrclib_item(entry, None))
            .collect();

        drop(db);
        items
    }

    pub async fn lrclib_get_by_fields(
        state: &AppState,
        query: SearchQuery,
    ) -> Result<LrclibSongItem, AppError> {
        let db = state.db.load();

        let prepared = PreparedQuery::from_search_query(&query);
        let matched_songs = db.search_by_fields(&query);

        if matched_songs.is_empty() {
            return Err(AppError::LyricNotFound);
        }

        let best_match = matched_songs[0];
        let score = score_entry(&prepared, best_match);

        if score < MatchType::Medium {
            return Err(AppError::LyricNotFound);
        }

        let latest_song_cloned = best_match.clone();
        drop(db);

        let ttml_text = state
            .fetch_lyric_ttml(latest_song_cloned.filename.as_str())
            .await?;
        let item = map_to_lrclib_item(&latest_song_cloned, Some(ttml_text));
        Ok(item)
    }

    pub async fn lrclib_get_by_id(state: &AppState, id: u64) -> Result<LrclibSongItem, AppError> {
        let db = state.db.load();

        let idx = db.id_idx.get(&id).copied().ok_or(AppError::LyricNotFound)?;
        let song_cloned = db.entries[idx].clone();
        drop(db);

        let ttml_text = state
            .fetch_lyric_ttml(song_cloned.filename.as_str())
            .await?;
        let item = map_to_lrclib_item(&song_cloned, Some(ttml_text));
        Ok(item)
    }
}
