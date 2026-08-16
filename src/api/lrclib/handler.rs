use axum::{
    Json,
    extract::{
        Path,
        RawQuery,
        State,
    },
    response::IntoResponse,
};

use crate::{
    api::lrclib::{
        dto::{
            LrclibSongItem,
            map_to_lrclib_item,
        },
        extractor::{
            extract_lrclib_get_query,
            extract_lrclib_search_query,
        },
    },
    core::{
        LyricId,
        error::AppError,
        state::AppState,
    },
    services::lyric_service,
};

pub async fn handle_search(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, AppError> {
    let (query, pagination) = extract_lrclib_search_query(raw_query.as_deref().unwrap_or(""))?;
    let hits = lyric_service::lrclib_search(&state, &query, pagination).await;

    let items: Vec<LrclibSongItem> = hits
        .into_iter()
        .map(|(entry, formatted)| map_to_lrclib_item(&entry, formatted.as_ref()))
        .collect();

    Ok(Json(items))
}

pub async fn handle_get(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, AppError> {
    let query = extract_lrclib_get_query(raw_query.as_deref().unwrap_or(""))?;
    let (entry, formatted) = lyric_service::lrclib_get_by_fields(&state, query).await?;

    Ok(Json(map_to_lrclib_item(&entry, Some(&formatted))))
}

pub async fn handle_get_by_id(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<impl IntoResponse, AppError> {
    let lyric_id = LyricId::from_u64(id)?;
    let (entry, formatted) = lyric_service::lrclib_get_by_id(&state, lyric_id).await?;
    Ok(Json(map_to_lrclib_item(&entry, Some(&formatted))))
}
