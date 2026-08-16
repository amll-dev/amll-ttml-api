use url::form_urlencoded;

use crate::core::{
    LyricId,
    error::AppError,
    models::{
        IdQuery,
        SearchQuery,
    },
    pagination::Pagination,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFieldTarget {
    GlobalKeyword,
    TrackName,
    ArtistName,
    AlbumName,
    LyricText,
    AuthorId,
    AuthorUsername,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchRequirement {
    /// 允许任意搜索参数，若 `q` 与非 `q` 共同存在则丢弃 `q`，用于原生接口
    AnyParamWithQFallback,
    /// 必须同时包含指定的每一个字段 (如 `track_name` 和 `artist_name`)，用于 `LrcLib` 接口
    ExactRequired(&'static [SearchFieldTarget]),
}

pub struct SearchDialect {
    pub field_map: &'static [(&'static str, SearchFieldTarget)],
    pub requirement: SearchRequirement,
}

/// `/v1/lyrics/search` 的请求参数映射
pub static NATIVE_SEARCH_DIALECT: SearchDialect = SearchDialect {
    field_map: &[
        ("q", SearchFieldTarget::GlobalKeyword),
        ("musicName", SearchFieldTarget::TrackName),
        ("artistName", SearchFieldTarget::ArtistName),
        ("albumName", SearchFieldTarget::AlbumName),
        ("lyricText", SearchFieldTarget::LyricText),
        ("authorId", SearchFieldTarget::AuthorId),
        ("authorUsername", SearchFieldTarget::AuthorUsername),
    ],
    requirement: SearchRequirement::AnyParamWithQFallback,
};

/// `/v1/lrclib/search` 的请求参数映射
pub static LRCLIB_SEARCH_DIALECT: SearchDialect = SearchDialect {
    field_map: &[
        ("q", SearchFieldTarget::GlobalKeyword),
        ("track_name", SearchFieldTarget::TrackName),
        ("artist_name", SearchFieldTarget::ArtistName),
        ("album_name", SearchFieldTarget::AlbumName),
    ],
    requirement: SearchRequirement::AnyParamWithQFallback,
};

/// `/v1/lrclib/get` 的请求参数映射
pub static LRCLIB_GET_DIALECT: SearchDialect = SearchDialect {
    field_map: &[
        ("track_name", SearchFieldTarget::TrackName),
        ("artist_name", SearchFieldTarget::ArtistName),
        ("album_name", SearchFieldTarget::AlbumName),
    ],
    requirement: SearchRequirement::ExactRequired(&[
        SearchFieldTarget::TrackName,
        SearchFieldTarget::ArtistName,
    ]),
};

pub fn parse_search_query(
    query_str: &str,
    dialect: &SearchDialect,
) -> Result<(SearchQuery, Pagination), AppError> {
    let parsed = parse_query_internal(query_str, dialect);

    if !parsed.has_any_param {
        return Err(AppError::BadRequest(
            "Missing valid search parameters.".into(),
        ));
    }

    let pagination =
        Pagination::from_raw(parsed.page_raw.as_deref(), parsed.page_size_raw.as_deref())?;
    Ok((parsed.query, pagination))
}

pub fn parse_search_query_exact(
    query_str: &str,
    dialect: &SearchDialect,
) -> Result<SearchQuery, AppError> {
    let parsed = parse_query_internal(query_str, dialect);

    if let SearchRequirement::ExactRequired(reqs) = dialect.requirement {
        for req in reqs {
            let present = match req {
                SearchFieldTarget::TrackName => parsed.query.track_name.is_some(),
                SearchFieldTarget::ArtistName => parsed.query.artist_name.is_some(),
                SearchFieldTarget::AlbumName => parsed.query.album_name.is_some(),
                SearchFieldTarget::GlobalKeyword => parsed.query.global_keyword.is_some(),
                SearchFieldTarget::LyricText => parsed.query.lyric_text.is_some(),
                SearchFieldTarget::AuthorId => parsed.query.author_id.is_some(),
                SearchFieldTarget::AuthorUsername => parsed.query.author_username.is_some(),
            };
            if !present {
                return Err(AppError::BadRequest(
                    "Both 'track_name' and 'artist_name' are required for precise matching.".into(),
                ));
            }
        }
    }

    Ok(parsed.query)
}

struct ParsedQueryParams {
    pub query: SearchQuery,
    pub has_any_param: bool,
    pub page_raw: Option<String>,
    pub page_size_raw: Option<String>,
}

fn parse_query_internal(query_str: &str, dialect: &SearchDialect) -> ParsedQueryParams {
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

        let key_str = k.as_ref();

        if key_str == "page" {
            page_raw = Some(val);
            continue;
        }
        if key_str == "pageSize" {
            page_size_raw = Some(val);
            continue;
        }

        if let Some(&target) =
            dialect.field_map.iter().find_map(
                |(name, target)| {
                    if *name == key_str { Some(target) } else { None }
                },
            )
        {
            match target {
                SearchFieldTarget::GlobalKeyword => {
                    query.global_keyword = Some(val);
                    has_q = true;
                }
                SearchFieldTarget::TrackName => {
                    query.track_name = Some(val);
                    has_non_q_param = true;
                }
                SearchFieldTarget::ArtistName => {
                    query.artist_name = Some(val);
                    has_non_q_param = true;
                }
                SearchFieldTarget::AlbumName => {
                    query.album_name = Some(val);
                    has_non_q_param = true;
                }
                SearchFieldTarget::LyricText => {
                    query.lyric_text = Some(val);
                    has_non_q_param = true;
                }
                SearchFieldTarget::AuthorId => {
                    query.author_id = Some(val);
                    has_non_q_param = true;
                }
                SearchFieldTarget::AuthorUsername => {
                    query.author_username = Some(val);
                    has_non_q_param = true;
                }
            }
        }
    }

    if dialect.requirement == SearchRequirement::AnyParamWithQFallback && has_q && has_non_q_param {
        query.global_keyword = None;
    }

    let has_any_param = has_q || has_non_q_param;

    ParsedQueryParams {
        query,
        has_any_param,
        page_raw,
        page_size_raw,
    }
}

pub struct GetQuery {
    pub id_query: IdQuery,
    pub format: String,
}

pub fn parse_get_query(query_str: &str) -> Result<GetQuery, AppError> {
    let mut query = IdQuery::default();
    let mut has_param = false;
    let mut format = String::from("ttml");

    for (k, v) in form_urlencoded::parse(query_str.as_bytes()) {
        let val = v.into_owned();
        if val.trim().is_empty() {
            continue;
        }
        match k.as_ref() {
            "id" => {
                let parsed_id = LyricId::parse(&val)?;
                query.id = Some(parsed_id);
                has_param = true;
            }
            "filename" => {
                query.filename = Some(val);
                has_param = true;
            }
            "ncmMusicId" => {
                query.ncm_music_ids.push(val);
                has_param = true;
            }
            "qqMusicId" => {
                query.qq_music_ids.push(val);
                has_param = true;
            }
            "appleMusicId" => {
                query.apple_music_ids.push(val);
                has_param = true;
            }
            "spotifyId" => {
                query.spotify_ids.push(val);
                has_param = true;
            }
            "isrc" => {
                query.isrcs.push(val);
                has_param = true;
            }
            "format" => {
                format = val;
            }
            _ => {}
        }
    }

    if format != "ttml" {
        return Err(AppError::BadRequest(format!(
            "Unsupported format: '{format}'. Only 'ttml' is currently supported."
        )));
    }

    #[expect(clippy::case_sensitive_file_extension_comparisons)]
    if let Some(ref filename) = query.filename
        && !filename.ends_with(".ttml")
    {
        return Err(AppError::BadRequest(format!(
            "Invalid filename: '{filename}'. Must end with '.ttml'."
        )));
    }

    if has_param {
        Ok(GetQuery {
            id_query: query,
            format,
        })
    } else {
        Err(AppError::BadRequest(
            "At least one parameter is required (id, filename, ncmMusicId, qqMusicId, appleMusicId, spotifyId, isrc).".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Native Search 测试组 ---

    #[test]
    fn native_search_no_params_returns_error() {
        let result = parse_search_query("", &NATIVE_SEARCH_DIALECT);
        assert!(result.is_err());
        if let Err(AppError::BadRequest(msg)) = result {
            assert_eq!(msg, "Missing valid search parameters.");
        }
    }

    #[test]
    fn native_search_empty_string_params_returns_error() {
        let result = parse_search_query("musicName=&artistName=", &NATIVE_SEARCH_DIALECT);
        assert!(result.is_err());
    }

    #[test]
    fn native_search_q_only() {
        let (query, pagination) =
            parse_search_query("q=Taylor+Swift", &NATIVE_SEARCH_DIALECT).unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("Taylor Swift"));
        assert!(query.track_name.is_none());
        assert_eq!(pagination.page, 1);
        assert_eq!(pagination.page_size, 50);
    }

    #[test]
    fn native_search_music_name_only() {
        let (query, _) = parse_search_query("musicName=ME!", &NATIVE_SEARCH_DIALECT).unwrap();
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
    }

    #[test]
    fn native_search_q_with_non_q_ignores_q() {
        let (query, _) =
            parse_search_query("q=Taylor+Swift&musicName=ME!", &NATIVE_SEARCH_DIALECT).unwrap();
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
    }

    #[test]
    fn native_search_multiple_non_q_params_preserved() {
        let (query, _) = parse_search_query(
            "musicName=ME!&artistName=Taylor+Swift&authorId=108002475",
            &NATIVE_SEARCH_DIALECT,
        )
        .unwrap();
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
        assert_eq!(query.artist_name.as_deref(), Some("Taylor Swift"));
        assert_eq!(query.author_id.as_deref(), Some("108002475"));
    }

    #[test]
    fn native_search_unknown_params_ignored() {
        let (query, _) =
            parse_search_query("q=hello&unknown=value", &NATIVE_SEARCH_DIALECT).unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
    }

    #[test]
    fn native_search_empty_value_skipped() {
        let (query, _) = parse_search_query("musicName=&q=hello", &NATIVE_SEARCH_DIALECT).unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
        assert!(query.track_name.is_none());
    }

    #[test]
    fn native_search_whitespace_only_value_skipped() {
        let (query, _) = parse_search_query("musicName=+&q=hello", &NATIVE_SEARCH_DIALECT).unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
        assert!(query.track_name.is_none());
    }

    #[test]
    fn native_search_all_params_populated() {
        let (query, _) = parse_search_query(
            "q=ignored&musicName=a&artistName=b&albumName=c&authorId=d&authorUsername=e",
            &NATIVE_SEARCH_DIALECT,
        )
        .unwrap();
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("a"));
        assert_eq!(query.artist_name.as_deref(), Some("b"));
        assert_eq!(query.album_name.as_deref(), Some("c"));
        assert_eq!(query.author_id.as_deref(), Some("d"));
        assert_eq!(query.author_username.as_deref(), Some("e"));
    }

    #[test]
    fn native_search_page_and_page_size_parsed() {
        let (query, pagination) =
            parse_search_query("q=hello&page=3&pageSize=20", &NATIVE_SEARCH_DIALECT).unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
        assert_eq!(pagination.page, 3);
        assert_eq!(pagination.page_size, 20);
    }

    #[test]
    fn native_search_page_params_do_not_set_has_non_q_flag() {
        let result = parse_search_query("page=2&pageSize=10", &NATIVE_SEARCH_DIALECT);
        assert!(result.is_err());
    }

    #[test]
    fn native_search_page_params_do_not_trigger_q_discard() {
        let (query, pagination) =
            parse_search_query("q=love&page=2", &NATIVE_SEARCH_DIALECT).unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("love"));
        assert_eq!(pagination.page, 2);
    }

    #[test]
    fn native_search_invalid_page_returns_error() {
        let result = parse_search_query("q=hello&page=0", &NATIVE_SEARCH_DIALECT);
        assert!(result.is_err());
    }

    #[test]
    fn native_search_invalid_page_size_returns_error() {
        let result = parse_search_query("q=hello&pageSize=200", &NATIVE_SEARCH_DIALECT);
        assert!(result.is_err());
    }

    // --- LRCLIB Search 测试组 ---

    #[test]
    fn lrclib_search_no_params_returns_error() {
        let result = parse_search_query("", &LRCLIB_SEARCH_DIALECT);
        assert!(result.is_err());
    }

    #[test]
    fn lrclib_search_q_only_success() {
        let (query, pagination) =
            parse_search_query("q=Taylor+Swift", &LRCLIB_SEARCH_DIALECT).unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("Taylor Swift"));
        assert!(query.track_name.is_none());
        assert_eq!(pagination.page, 1);
        assert_eq!(pagination.page_size, 50);
    }

    #[test]
    fn lrclib_search_snake_case_params_success() {
        let (query, _) = parse_search_query(
            "track_name=ME!&artist_name=Taylor+Swift",
            &LRCLIB_SEARCH_DIALECT,
        )
        .unwrap();
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
        assert_eq!(query.artist_name.as_deref(), Some("Taylor Swift"));
    }

    #[test]
    fn lrclib_search_ignores_duration_and_unsupported() {
        let (query, _) = parse_search_query(
            "track_name=ME!&duration=193&unknown=abc",
            &LRCLIB_SEARCH_DIALECT,
        )
        .unwrap();
        assert_eq!(query.track_name.as_deref(), Some("ME!"));
    }

    #[test]
    fn lrclib_search_q_is_ignored_if_specific_fields_exist() {
        let (query, _) =
            parse_search_query("q=hello&track_name=world", &LRCLIB_SEARCH_DIALECT).unwrap();
        assert!(query.global_keyword.is_none());
        assert_eq!(query.track_name.as_deref(), Some("world"));
    }

    #[test]
    fn lrclib_search_page_and_page_size_parsed() {
        let (query, pagination) =
            parse_search_query("q=hello&page=2&pageSize=25", &LRCLIB_SEARCH_DIALECT).unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("hello"));
        assert_eq!(pagination.page, 2);
        assert_eq!(pagination.page_size, 25);
    }

    #[test]
    fn lrclib_search_page_params_do_not_set_has_param_flag() {
        let result = parse_search_query("page=3&pageSize=10", &LRCLIB_SEARCH_DIALECT);
        assert!(result.is_err());
    }

    #[test]
    fn lrclib_search_page_params_do_not_trigger_q_discard() {
        let (query, pagination) =
            parse_search_query("q=love&page=3", &LRCLIB_SEARCH_DIALECT).unwrap();
        assert_eq!(query.global_keyword.as_deref(), Some("love"));
        assert_eq!(pagination.page, 3);
    }

    // --- LRCLIB Get 精准匹配测试组 ---

    #[test]
    fn lrclib_get_missing_artist_returns_error() {
        let result = parse_search_query_exact("track_name=ME!", &LRCLIB_GET_DIALECT);
        assert!(result.is_err());
        if let Err(AppError::BadRequest(msg)) = result {
            assert!(msg.contains("Both 'track_name' and 'artist_name' are required"));
        }
    }

    #[test]
    fn lrclib_get_missing_track_returns_error() {
        let result = parse_search_query_exact("artist_name=Taylor", &LRCLIB_GET_DIALECT);
        assert!(result.is_err());
    }

    #[test]
    fn lrclib_get_valid_params_success() {
        let result =
            parse_search_query_exact("track_name=ME!&artist_name=Taylor", &LRCLIB_GET_DIALECT)
                .unwrap();
        assert_eq!(result.track_name.as_deref(), Some("ME!"));
        assert_eq!(result.artist_name.as_deref(), Some("Taylor"));
    }

    // --- Get ID 测试组 ---

    #[test]
    fn get_no_params_returns_error() {
        let result = parse_get_query("");
        assert!(result.is_err());
    }

    #[test]
    fn get_empty_id_returns_error() {
        let result = parse_get_query("spotifyId=");
        assert!(result.is_err());
    }

    #[test]
    fn get_format_only_returns_error() {
        let result = parse_get_query("format=ttml");
        assert!(result.is_err());
    }

    #[test]
    fn get_single_spotify_id() {
        let result = parse_get_query("spotifyId=abc123").unwrap();
        assert_eq!(result.id_query.spotify_ids, vec!["abc123"]);
        assert_eq!(result.format, "ttml");
    }

    #[test]
    fn get_default_format_is_ttml() {
        let result = parse_get_query("ncmMusicId=111").unwrap();
        assert_eq!(result.format, "ttml");
    }

    #[test]
    fn get_unsupported_format_returns_error() {
        let result = parse_get_query("spotifyId=abc&format=lrc");
        assert!(result.is_err());
    }

    #[test]
    fn get_multiple_ids_same_type() {
        let result = parse_get_query("ncmMusicId=111&ncmMusicId=222").unwrap();
        assert_eq!(result.id_query.ncm_music_ids, vec!["111", "222"]);
    }

    #[test]
    fn get_multiple_ids_different_types() {
        let result = parse_get_query("ncmMusicId=111&spotifyId=abc&isrc=XYZ").unwrap();
        assert_eq!(result.id_query.ncm_music_ids, vec!["111"]);
        assert_eq!(result.id_query.spotify_ids, vec!["abc"]);
        assert_eq!(result.id_query.isrcs, vec!["XYZ"]);
    }

    #[test]
    fn get_all_id_types() {
        let result =
            parse_get_query("ncmMusicId=a&qqMusicId=b&appleMusicId=c&spotifyId=d&isrc=e").unwrap();
        assert_eq!(result.id_query.ncm_music_ids, vec!["a"]);
        assert_eq!(result.id_query.qq_music_ids, vec!["b"]);
        assert_eq!(result.id_query.apple_music_ids, vec!["c"]);
        assert_eq!(result.id_query.spotify_ids, vec!["d"]);
        assert_eq!(result.id_query.isrcs, vec!["e"]);
    }

    #[test]
    fn get_filename_only() {
        let result = parse_get_query("filename=1768754400682-250306205-r6IrpmBd.ttml").unwrap();
        assert_eq!(
            result.id_query.filename,
            Some("1768754400682-250306205-r6IrpmBd.ttml".into())
        );
        assert!(result.id_query.ncm_music_ids.is_empty());
    }

    #[test]
    fn get_filename_with_other_ids_ignored() {
        let result = parse_get_query("filename=a.ttml&ncmMusicId=111").unwrap();
        assert_eq!(result.id_query.filename, Some("a.ttml".into()));
        assert_eq!(result.id_query.ncm_music_ids, vec!["111"]);
    }

    #[test]
    fn get_empty_filename_returns_error() {
        let result = parse_get_query("filename=");
        assert!(result.is_err());
    }

    #[test]
    fn get_invalid_filename_extension_returns_error() {
        let result = parse_get_query("filename=1768754400682-250306205-r6IrpmBd.lrc");
        assert!(result.is_err());
    }

    #[test]
    fn get_valid_id_parsed_correctly() {
        let result = parse_get_query("id=269710089745311").unwrap();
        assert_eq!(
            result.id_query.id,
            Some(LyricId::from_u64(269_710_089_745_311).unwrap())
        );
    }

    #[test]
    fn get_invalid_id_format_returns_error() {
        let result1 = parse_get_query("id=abc");
        assert!(result1.is_err());

        let result2 = parse_get_query("id=-123");
        assert!(result2.is_err());
    }

    #[test]
    fn get_id_with_other_params() {
        let result = parse_get_query("id=12345&ncmMusicId=111").unwrap();
        assert_eq!(result.id_query.id, Some(LyricId::from_u64(12345).unwrap()));
        assert_eq!(result.id_query.ncm_music_ids, vec!["111"]);
    }
}
