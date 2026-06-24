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

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn no_params_returns_error() {
        let result = extract_search_query(&url("https://example.com/api/v1/lyrics/search"));
        assert!(result.is_err());
    }

    #[test]
    fn empty_string_params_returns_error() {
        let result = extract_search_query(&url(
            "https://example.com/api/v1/lyrics/search?musicName=&artistName=",
        ));
        assert!(result.is_err());
    }

    #[test]
    fn q_only() {
        let result = extract_search_query(&url(
            "https://example.com/api/v1/lyrics/search?q=Taylor+Swift",
        ))
        .unwrap();
        assert_eq!(result.global_keyword.as_deref(), Some("Taylor Swift"));
        assert!(result.track_name.is_none());
        assert!(result.artist_name.is_none());
    }

    #[test]
    fn music_name_only() {
        let result = extract_search_query(&url(
            "https://example.com/api/v1/lyrics/search?musicName=ME!",
        ))
        .unwrap();
        assert!(result.global_keyword.is_none());
        assert_eq!(result.track_name.as_deref(), Some("ME!"));
    }

    #[test]
    fn q_with_non_q_ignores_q() {
        let result = extract_search_query(&url(
            "https://example.com/api/v1/lyrics/search?q=Taylor+Swift&musicName=ME!",
        ))
        .unwrap();
        assert!(result.global_keyword.is_none());
        assert_eq!(result.track_name.as_deref(), Some("ME!"));
    }

    #[test]
    fn multiple_non_q_params_preserved() {
        let result = extract_search_query(&url(
            "https://example.com/api/v1/lyrics/search?musicName=ME!&artistName=Taylor+Swift&authorId=108002475",
        ))
        .unwrap();
        assert_eq!(result.track_name.as_deref(), Some("ME!"));
        assert_eq!(result.artist_name.as_deref(), Some("Taylor Swift"));
        assert_eq!(result.author_id.as_deref(), Some("108002475"));
    }

    #[test]
    fn unknown_params_ignored() {
        let result = extract_search_query(&url(
            "https://example.com/api/v1/lyrics/search?q=hello&unknown=value",
        ))
        .unwrap();
        assert_eq!(result.global_keyword.as_deref(), Some("hello"));
    }

    #[test]
    fn empty_value_skipped() {
        let result = extract_search_query(&url(
            "https://example.com/api/v1/lyrics/search?musicName=&q=hello",
        ))
        .unwrap();
        assert_eq!(result.global_keyword.as_deref(), Some("hello"));
        assert!(result.track_name.is_none());
    }

    #[test]
    fn whitespace_only_value_skipped() {
        let result = extract_search_query(&url(
            "https://example.com/api/v1/lyrics/search?musicName=+&q=hello",
        ))
        .unwrap();
        assert_eq!(result.global_keyword.as_deref(), Some("hello"));
        assert!(result.track_name.is_none());
    }

    #[test]
    fn all_params_populated() {
        let result = extract_search_query(&url(
            "https://example.com/api/v1/lyrics/search?q=ignored&musicName=a&artistName=b&albumName=c&authorId=d&authorUsername=e",
        ))
        .unwrap();
        // q should be ignored
        assert!(result.global_keyword.is_none());
        assert_eq!(result.track_name.as_deref(), Some("a"));
        assert_eq!(result.artist_name.as_deref(), Some("b"));
        assert_eq!(result.album_name.as_deref(), Some("c"));
        assert_eq!(result.author_id.as_deref(), Some("d"));
        assert_eq!(result.author_username.as_deref(), Some("e"));
    }
}
