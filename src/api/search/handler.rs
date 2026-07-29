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
    let query = extract_search_query(raw_query.as_deref().unwrap_or(""))?;
    let max_results = 50;
    let result = LyricService::search_lyric(&state, &query, max_results).await;

    Ok(Json(result))
}
