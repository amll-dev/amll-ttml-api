use worker::Url;

use crate::core::{
    error::AppError,
    models::SearchQuery,
};

pub fn extract_search_query(url: &Url) -> Result<SearchQuery, AppError> {
    let mut query = SearchQuery::default();
    let mut has_non_q_param = false;
    let mut has_q = false;

    for (k, v) in url.query_pairs() {
        let val = v.into_owned();
        if val.trim().is_empty() {
            continue;
        }
        match k.as_ref() {
            "q" => {
                query.global_keyword = Some(val);
                has_q = true;
            }
            "musicName" => {
                query.track_name = Some(val);
                has_non_q_param = true;
            }
            "artistName" => {
                query.artist_name = Some(val);
                has_non_q_param = true;
            }
            "albumName" => {
                query.album_name = Some(val);
                has_non_q_param = true;
            }
            "authorId" => {
                query.author_id = Some(val);
                has_non_q_param = true;
            }
            "authorUsername" => {
                query.author_username = Some(val);
                has_non_q_param = true;
            }
            _ => {}
        }
    }

    // 如果同时传入了 q 和其他参数，忽略 q
    if has_q && has_non_q_param {
        query.global_keyword = None;
    }

    let has_any = has_q || has_non_q_param;
    if has_any {
        Ok(query)
    } else {
        Err(AppError::BadRequest(
            "Missing valid search parameters.".into(),
        ))
    }
}
