use axum::{
    Json,
    extract::{
        RawQuery,
        State,
    },
    response::IntoResponse,
};

use crate::{
    api::search::extractor::extract_search_query,
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
    let (query, pagination) = extract_search_query(raw_query.as_deref().unwrap_or(""))?;
    let result = LyricService::search_lyric(&state, &query, pagination).await;

    Ok(Json(result))
}
