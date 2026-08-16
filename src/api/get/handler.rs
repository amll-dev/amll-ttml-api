use axum::{
    extract::{
        RawQuery,
        State,
    },
    response::IntoResponse,
};

use crate::{
    api::{
        get::extractor::extract_get_query,
        shared::dto::{
            ApiSuccess,
            map_song_to_item,
        },
    },
    core::{
        error::AppError,
        state::AppState,
    },
    services::lyric_service,
};

pub async fn handle_get(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, AppError> {
    let get_query = extract_get_query(raw_query.as_deref().unwrap_or(""))?;

    let (entry, ttml_text) = lyric_service::get_lyric(&state, get_query.id_query).await?;

    Ok(ApiSuccess(map_song_to_item(
        &entry,
        Some(ttml_text),
        Some(get_query.format),
        None,
    )))
}
