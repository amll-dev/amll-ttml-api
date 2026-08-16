use axum::{
    extract::{
        RawQuery,
        State,
    },
    response::IntoResponse,
};

use crate::{
    api::{
        search::extractor::extract_search_query,
        shared::dto::{
            ApiSuccess,
            MatchContext,
            SearchData,
            map_song_to_item,
        },
    },
    core::error::AppError,
    services::{
        AppState,
        lyric_service,
    },
};

pub async fn handle_search(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, AppError> {
    let (query, pagination) = extract_search_query(raw_query.as_deref().unwrap_or(""))?;
    let result = lyric_service::search_lyric(&state.store, &query, pagination).await;

    let items = result
        .items
        .into_iter()
        .map(|hit| {
            let match_context = hit
                .lyric_hit
                .and_then(|lyric| lyric.snippet)
                .map(|snippet| MatchContext {
                    snippet: Some(snippet),
                });
            map_song_to_item(&hit.entry, None, None, match_context)
        })
        .collect();

    Ok(ApiSuccess(SearchData {
        items,
        pagination: result.pagination,
    }))
}
