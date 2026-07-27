use axum::{
    Json,
    extract::{
        RawQuery,
        State,
    },
    response::IntoResponse,
};

use crate::{
    api::get::extractor::extract_get_query,
    core::{
        error::AppError,
        state::AppState,
    },
    services::lyric_service::LyricService,
};

pub async fn handle_get(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, AppError> {
    let get_query = extract_get_query(raw_query.as_deref().unwrap_or(""))?;

    let result = LyricService::get_lyric(&state, get_query.id_query, get_query.format).await?;
    Ok(Json(result))
}
