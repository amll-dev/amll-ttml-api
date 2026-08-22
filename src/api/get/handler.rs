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
        get::extractor::extract_get_query,
        shared::{
            cache::{
                EXACT_CACHE_CONTROL,
                WEAK_CACHE_CONTROL,
            },
            dto::{
                ApiSuccess,
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

pub async fn handle_get(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, AppError> {
    let get_query = extract_get_query(raw_query.as_deref().unwrap_or(""))?;
    let is_exact = get_query.id_query.is_exact();

    let (entry, ttml_text) = lyric_service::get_lyric(&state.store, get_query.id_query).await?;

    let cache_control = if is_exact {
        EXACT_CACHE_CONTROL
    } else {
        WEAK_CACHE_CONTROL
    };

    Ok((
        [(header::CACHE_CONTROL, cache_control)],
        ApiSuccess(map_song_to_item(
            &entry,
            Some(ttml_text),
            Some(get_query.format),
            None,
        )),
    ))
}
