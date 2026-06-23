use worker::Url;

use crate::core::{
    error::AppError,
    models::SearchQuery,
};

pub fn extract_search_query(url: &Url) -> Result<SearchQuery, AppError> {
    let mut query = SearchQuery::default();
    let mut has_param = false;

    for (k, v) in url.query_pairs() {
        let val = Some(v.into_owned());
        match k.as_ref() {
            "q" => {
                query.global_keyword = val;
                has_param = true;
            }
            "trackName" => {
                query.track_name = val;
                has_param = true;
            }
            "artistName" => {
                query.artist_name = val;
                has_param = true;
            }
            "albumName" => {
                query.album_name = val;
                has_param = true;
            }
            "authorId" => {
                query.author_id = val;
                has_param = true;
            }
            "authorUsername" => {
                query.author_username = val;
                has_param = true;
            }
            _ => {}
        }
    }

    if has_param {
        Ok(query)
    } else {
        Err(AppError::BadRequest(
            "Missing valid search parameters.".into(),
        ))
    }
}
