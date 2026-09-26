use axum::{
    extract::{
        RawQuery,
        State,
    },
    http::header,
    response::IntoResponse,
};

use crate::{
    api::{
        list::extractor::extract_list_query,
        shared::{
            cache::SEARCH_CACHE_CONTROL,
            dto::{
                ApiSuccess,
                SearchData,
                map_song_to_item,
            },
        },
    },
    core::error::AppError,
    services::{
        AppState,
        lyric_service,
    },
};

pub async fn handle_list(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, AppError> {
    let query = extract_list_query(raw_query.as_deref().unwrap_or(""))?;
    let result = lyric_service::list_lyrics(&state.store, query).await;

    let items = result
        .items
        .iter()
        .map(|entry| map_song_to_item(entry, None, None, None))
        .collect();

    Ok((
        [(header::CACHE_CONTROL, SEARCH_CACHE_CONTROL)],
        ApiSuccess(SearchData {
            items,
            pagination: result.pagination,
        }),
    ))
}
