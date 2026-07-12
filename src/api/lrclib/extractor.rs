use worker::Url;

use crate::core::{
    error::AppError,
    models::SearchQuery,
};

pub fn extract_lrclib_search_query(url: &Url) -> Result<SearchQuery, AppError> {
    let mut query = SearchQuery::default();
    let mut has_param = false;
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
            _ => {}
        }
    }

    if has_q && has_param {
        query.global_keyword = None;
    }

    if has_q || has_param {
        Ok(query)
    } else {
        Err(AppError::BadRequest(
            "Missing valid search parameters.".into(),
        ))
    }
}

pub fn extract_lrclib_get_query(url: &Url) -> Result<SearchQuery, AppError> {
    let mut query = SearchQuery::default();

    for (k, v) in url.query_pairs() {
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

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn search_no_params_returns_error() {
        let result = extract_lrclib_search_query(&url("https://example.com/api/v1/lrclib/search"));
        assert!(result.is_err());
    }

    #[test]
    fn search_q_only_success() {
        let result = extract_lrclib_search_query(&url(
            "https://example.com/api/v1/lrclib/search?q=Taylor+Swift",
        ))
        .unwrap();
        assert_eq!(result.global_keyword.as_deref(), Some("Taylor Swift"));
        assert!(result.track_name.is_none());
    }

    #[test]
    fn search_snake_case_params_success() {
        let result = extract_lrclib_search_query(&url(
            "https://example.com/api/v1/lrclib/search?track_name=ME!&artist_name=Taylor+Swift",
        ))
        .unwrap();
        assert!(result.global_keyword.is_none());
        assert_eq!(result.track_name.as_deref(), Some("ME!"));
        assert_eq!(result.artist_name.as_deref(), Some("Taylor Swift"));
    }

    #[test]
    fn search_ignores_duration_and_unsupported() {
        let result = extract_lrclib_search_query(&url(
            "https://example.com/api/v1/lrclib/search?track_name=ME!&duration=193&unknown=abc",
        ))
        .unwrap();
        assert_eq!(result.track_name.as_deref(), Some("ME!"));
    }

    #[test]
    fn search_q_is_ignored_if_specific_fields_exist() {
        let result = extract_lrclib_search_query(&url(
            "https://example.com/api/v1/lrclib/search?q=hello&track_name=world",
        ))
        .unwrap();
        assert!(result.global_keyword.is_none());
        assert_eq!(result.track_name.as_deref(), Some("world"));
    }

    #[test]
    fn get_missing_artist_returns_error() {
        let result =
            extract_lrclib_get_query(&url("https://example.com/api/v1/lrclib/get?track_name=ME!"));
        assert!(result.is_err());
        if let Err(AppError::BadRequest(msg)) = result {
            assert!(msg.contains("Both 'track_name' and 'artist_name' are required"));
        }
    }

    #[test]
    fn get_missing_track_returns_error() {
        let result = extract_lrclib_get_query(&url(
            "https://example.com/api/v1/lrclib/get?artist_name=Taylor",
        ));
        assert!(result.is_err());
    }

    #[test]
    fn get_valid_params_success() {
        let result = extract_lrclib_get_query(&url(
            "https://example.com/api/v1/lrclib/get?track_name=ME!&artist_name=Taylor",
        ))
        .unwrap();
        assert_eq!(result.track_name.as_deref(), Some("ME!"));
        assert_eq!(result.artist_name.as_deref(), Some("Taylor"));
    }
}
