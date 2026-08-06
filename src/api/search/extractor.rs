use url::form_urlencoded;

use crate::{
    api::shared::pagination::Pagination,
    core::{
        error::AppError,
        models::SearchQuery,
    },
};

pub fn extract_search_query(query_str: &str) -> Result<(SearchQuery, Pagination), AppError> {
    let mut query = SearchQuery::default();
    let mut has_non_q_param = false;
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
            "lyricText" => {
                query.lyric_text = Some(val);
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
            "page" => {
                page_raw = Some(val);
            }
            "pageSize" => {
                page_size_raw = Some(val);
            }
            _ => {}
        }
    }

    // 如果同时传入了 q 和其他参数，忽略 q
    if has_q && has_non_q_param {
        query.global_keyword = None;
    }

    let has_any = has_q || has_non_q_param;
    if !has_any {
        return Err(AppError::BadRequest(
            "Missing valid search parameters.".into(),
        ));
    }

    let pagination = Pagination::from_raw(page_raw.as_deref(), page_size_raw.as_deref())?;

    Ok((query, pagination))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_params_returns_error() {
        let result = extract_search_query("");
        assert!(result.is_err());
    }

    #[test]
    fn empty_string_params_returns_error() {
        let result = extract_search_query("musicName=&artistName=");
        assert!(result.is_err());
    }

    #[test]
    fn q_only() {
        let (query, pagination) = extract_search_query("q=Taylor+Swift").unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("Taylor Swift"));
        assert!(query.track_name.is_none());
        assert!(query.artist_name.is_none());
        assert_eq!(pagination.page, 1);
        assert_eq!(pagination.page_size, 50);
    }

    #[test]
    fn music_name_only() {
        let (query, _) = extract_search_query("musicName=ME!").unwrap();
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
    }

    #[test]
    fn q_with_non_q_ignores_q() {
        let (query, _) = extract_search_query("q=Taylor+Swift&musicName=ME!").unwrap();
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
    }

    #[test]
    fn multiple_non_q_params_preserved() {
        let (query, _) =
            extract_search_query("musicName=ME!&artistName=Taylor+Swift&authorId=108002475")
                .unwrap();
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
        assert_eq!(query.artist_name.as_deref(), Some("Taylor Swift"));
        assert_eq!(query.author_id.as_deref(), Some("108002475"));
    }

    #[test]
    fn unknown_params_ignored() {
        let (query, _) = extract_search_query("q=hello&unknown=value").unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
    }

    #[test]
    fn empty_value_skipped() {
        let (query, _) = extract_search_query("musicName=&q=hello").unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
        assert!(query.track_name.is_none());
    }

    #[test]
    fn whitespace_only_value_skipped() {
        let (query, _) = extract_search_query("musicName=+&q=hello").unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
        assert!(query.track_name.is_none());
    }

    #[test]
    fn all_params_populated() {
        let (query, _) = extract_search_query(
            "q=ignored&musicName=a&artistName=b&albumName=c&authorId=d&authorUsername=e",
        )
        .unwrap();
        // q should be ignored
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("a"));
        assert_eq!(query.artist_name.as_deref(), Some("b"));
        assert_eq!(query.album_name.as_deref(), Some("c"));
        assert_eq!(query.author_id.as_deref(), Some("d"));
        assert_eq!(query.author_username.as_deref(), Some("e"));
    }

    #[test]
    fn page_and_page_size_parsed() {
        let (query, pagination) = extract_search_query("q=hello&page=3&pageSize=20").unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
        assert_eq!(pagination.page, 3);
        assert_eq!(pagination.page_size, 20);
    }

    #[test]
    fn page_params_do_not_set_has_non_q_flag() {
        // Critical: page/pageSize alone should not satisfy "has_any"
        let result = extract_search_query("page=2&pageSize=10");
        assert!(result.is_err());
    }

    #[test]
    fn page_params_do_not_trigger_q_discard() {
        // Critical: q + page should keep q (page doesn't set has_non_q_param)
        let (query, pagination) = extract_search_query("q=love&page=2").unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("love"));
        assert_eq!(pagination.page, 2);
    }

    #[test]
    fn invalid_page_returns_error() {
        let result = extract_search_query("q=hello&page=0");
        assert!(result.is_err());
    }

    #[test]
    fn invalid_page_size_returns_error() {
        let result = extract_search_query("q=hello&pageSize=200");
        assert!(result.is_err());
    }
}
