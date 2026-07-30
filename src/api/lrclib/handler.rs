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
    api::lrclib::extractor::{
        extract_lrclib_get_query,
        extract_lrclib_search_query,
    },
    core::{
        error::AppError,
        state::AppState,
    },
    services::lyric_service::LyricService,
};

pub async fn handle_search(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, AppError> {
    let query = extract_lrclib_search_query(raw_query.as_deref().unwrap_or(""))?;
    let max_results = 50;
    let items = LyricService::lrclib_search(&state, &query, max_results).await;

    Ok(Json(items))
}

pub async fn handle_get(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, AppError> {
    let query = extract_lrclib_get_query(raw_query.as_deref().unwrap_or(""))?;
    let item = LyricService::lrclib_get_by_fields(&state, query).await?;

    Ok(Json(item))
}

pub async fn handle_get_by_id(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<impl IntoResponse, AppError> {
    let item = LyricService::lrclib_get_by_id(&state, id).await?;
    Ok(Json(item))
}
