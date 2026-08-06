use url::form_urlencoded;

use crate::{
    api::shared::pagination::Pagination,
    core::{
        error::AppError,
        models::SearchQuery,
    },
};

pub fn extract_lrclib_search_query(query_str: &str) -> Result<(SearchQuery, Pagination), AppError> {
    let mut query = SearchQuery::default();
    let mut has_param = false;
    let mut has_q = false;

    let mut page_raw: Option<String> = None;
    let mut page_size_raw: Option<String> = None;

    for (k, v) in form_urlencoded::parse(query_str.as_bytes()) {
        let val = v.into_owned();
        if val.trim().is_empty() {
            continue;
        }
        match k.as_ref() {
            "q" => {
                query.global_keyword = Some(val);
                has_q = true;
            }
            "track_name" => {
                query.track_name = Some(val);
                has_param = true;
            }
            "artist_name" => {
                query.artist_name = Some(val);
                has_param = true;
            }
            "album_name" => {
                query.album_name = Some(val);
                has_param = true;
            }
            "page" => {
                page_raw = Some(val);
            }
            "pageSize" => {
                page_size_raw = Some(val);
            }
            _ => {}
        }
    }

    if has_q && has_param {
        query.global_keyword = None;
    }

    if !(has_q || has_param) {
        return Err(AppError::BadRequest(
            "Missing valid search parameters.".into(),
        ));
    }

    let pagination = Pagination::from_raw(page_raw.as_deref(), page_size_raw.as_deref())?;

    Ok((query, pagination))
}

pub fn extract_lrclib_get_query(query_str: &str) -> Result<SearchQuery, AppError> {
    let mut query = SearchQuery::default();

    for (k, v) in form_urlencoded::parse(query_str.as_bytes()) {
        let val = v.into_owned();

        if val.trim().is_empty() {
            continue;
        }

        match k.as_ref() {
            "track_name" => query.track_name = Some(val),
            "artist_name" => query.artist_name = Some(val),
            "album_name" => query.album_name = Some(val),
            _ => {}
        }
    }

    if query.track_name.is_none() || query.artist_name.is_none() {
        return Err(AppError::BadRequest(
            "Both 'track_name' and 'artist_name' are required for precise matching.".into(),
        ));
    }

    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_no_params_returns_error() {
        let result = extract_lrclib_search_query("");
        assert!(result.is_err());
    }

    #[test]
    fn search_q_only_success() {
        let (query, pagination) = extract_lrclib_search_query("q=Taylor+Swift").unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("Taylor Swift"));
        assert!(query.track_name.is_none());
        assert_eq!(pagination.page, 1);
        assert_eq!(pagination.page_size, 50);
    }

    #[test]
    fn search_snake_case_params_success() {
        let (query, _) =
            extract_lrclib_search_query("track_name=ME!&artist_name=Taylor+Swift").unwrap();
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
        assert_eq!(query.artist_name.as_deref(), Some("Taylor Swift"));
    }

    #[test]
    fn search_ignores_duration_and_unsupported() {
        let (query, _) =
            extract_lrclib_search_query("track_name=ME!&duration=193&unknown=abc").unwrap();
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
    }

    #[test]
    fn search_q_is_ignored_if_specific_fields_exist() {
        let (query, _) = extract_lrclib_search_query("q=hello&track_name=world").unwrap();
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("world"));
    }

    #[test]
    fn search_page_and_page_size_parsed() {
        let (query, pagination) =
            extract_lrclib_search_query("q=hello&page=2&pageSize=25").unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
        assert_eq!(pagination.page, 2);
        assert_eq!(pagination.page_size, 25);
    }

    #[test]
    fn search_page_params_do_not_set_has_param_flag() {
        // Critical: page/pageSize alone should not satisfy "has_q || has_param"
        let result = extract_lrclib_search_query("page=3&pageSize=10");
        assert!(result.is_err());
    }

    #[test]
    fn search_page_params_do_not_trigger_q_discard() {
        // Critical: q + page should keep q (page doesn't set has_param)
        let (query, pagination) = extract_lrclib_search_query("q=love&page=3").unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("love"));
        assert_eq!(pagination.page, 3);
    }

    #[test]
    fn get_missing_artist_returns_error() {
        let result = extract_lrclib_get_query("track_name=ME!");
        assert!(result.is_err());
        if let Err(AppError::BadRequest(msg)) = result {
            assert!(msg.contains("Both 'track_name' and 'artist_name' are required"));
        }
    }

    #[test]
    fn get_missing_track_returns_error() {
        let result = extract_lrclib_get_query("artist_name=Taylor");
        assert!(result.is_err());
    }

    #[test]
    fn get_valid_params_success() {
        let result = extract_lrclib_get_query("track_name=ME!&artist_name=Taylor").unwrap();
        assert_eq!(result.track_name.as_deref(), Some("ME!"));
        assert_eq!(result.artist_name.as_deref(), Some("Taylor"));
    }
}
