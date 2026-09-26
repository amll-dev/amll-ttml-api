use url::form_urlencoded;

use crate::core::{
    LyricId,
    error::AppError,
    list_query::{
        Cursor,
        IdKind,
        IdKindSet,
        ListFilter,
        ListOrder,
        ListQuery,
        ListSort,
    },
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

/// `/v1/lyrics/list` 的 `hasId` / `missingId` 取值映射
///
/// 取值与 `/v1/lyrics/get` 的平台 ID 参数同名，两处指的是同一批标识
static ID_KIND_MAP: &[(&str, IdKind)] = &[
    ("ncmMusicId", IdKind::NcmMusicId),
    ("qqMusicId", IdKind::QqMusicId),
    ("appleMusicId", IdKind::AppleMusicId),
    ("spotifyId", IdKind::SpotifyId),
    ("isrc", IdKind::Isrc),
];

/// 报错消息里的取值清单，与 `sort` / `order` 一样硬编码
const ID_KIND_VALUES: &str = "'ncmMusicId', 'qqMusicId', 'appleMusicId', 'spotifyId', or 'isrc'";

/// 解析列表端点查询参数：分页、排序与结构化过滤
///
/// 列表端点不接受检索条件，无法识别的参数会被忽略。`sort` 默认使用 `createdAt`，`order` 默认使用
/// `desc`
///
/// 过滤维度之间、以及 `hasId` / `missingId` 各自的多个取值之间，全部是 AND
#[expect(clippy::too_many_lines)]
pub fn parse_list_query(query_str: &str) -> Result<ListQuery, AppError> {
    let mut page_raw: Option<String> = None;
    let mut page_size_raw: Option<String> = None;
    let mut sort_raw: Option<String> = None;
    let mut order_raw: Option<String> = None;
    let mut cursor_raw: Option<String> = None;
    let mut before_raw: Option<String> = None;
    let mut since_raw: Option<String> = None;
    let mut until_raw: Option<String> = None;
    let mut filter = ListFilter::default();

    for (key, value) in form_urlencoded::parse(query_str.as_bytes()) {
        let value = value.into_owned();
        if value.trim().is_empty() {
            continue;
        }

        match key.as_ref() {
            "page" => page_raw = Some(value),
            "pageSize" => page_size_raw = Some(value),
            "sort" => sort_raw = Some(value),
            "order" => order_raw = Some(value),
            "cursor" => cursor_raw = Some(value),
            "before" => before_raw = Some(value),
            "since" => since_raw = Some(value),
            "until" => until_raw = Some(value),
            "authorId" => filter.author_id = Some(value),
            "authorUsername" => filter.author_username = Some(value),
            "hasId" => collect_id_kinds("hasId", &value, &mut filter.has)?,
            "missingId" => collect_id_kinds("missingId", &value, &mut filter.missing)?,
            _ => {}
        }
    }

    let cursor_str = match (cursor_raw, before_raw) {
        (Some(c), Some(b)) if c != b => {
            return Err(AppError::BadRequest(
                "'cursor' and 'before' cannot both be specified with different values.".into(),
            ));
        }
        (Some(c), _) => Some(c),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    if page_raw.is_some() && cursor_str.is_some() {
        return Err(AppError::BadRequest(
            "'page' and 'cursor' cannot be used together.".into(),
        ));
    }

    let overlap = filter.has.intersection(filter.missing);
    if !overlap.is_empty() {
        return Err(AppError::BadRequest(format!(
            "'hasId' and 'missingId' must not overlap, but both list {}.",
            format_id_kinds(overlap)
        )));
    }

    let since = match since_raw.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Some(s.parse::<u64>().map_err(|_| {
            AppError::BadRequest(format!(
                "'since' must be a valid positive integer timestamp, got '{s}'."
            ))
        })?),
        _ => None,
    };
    let until = match until_raw.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Some(s.parse::<u64>().map_err(|_| {
            AppError::BadRequest(format!(
                "'until' must be a valid positive integer timestamp, got '{s}'."
            ))
        })?),
        _ => None,
    };

    if let (Some(s), Some(u)) = (since, until)
        && s > u
    {
        return Err(AppError::BadRequest(format!(
            "'since' ({s}) must not be greater than 'until' ({u})."
        )));
    }

    filter.since = since;
    filter.until = until;

    let pagination = Pagination::from_raw(page_raw.as_deref(), page_size_raw.as_deref())?;
    let sort = match sort_raw.as_deref().map(str::trim) {
        None => ListSort::default(),
        Some("createdAt") => ListSort::CreatedAt,
        Some("id") => ListSort::Id,
        Some("musicName") => ListSort::TrackName,
        Some("artistName") => ListSort::ArtistName,
        Some("albumName") => ListSort::AlbumName,
        Some(value) => {
            return Err(AppError::BadRequest(format!(
                "'sort' must be one of 'createdAt', 'id', 'musicName', 'artistName', or 'albumName', got '{value}'."
            )));
        }
    };
    let order = match order_raw.as_deref().map(str::trim) {
        None => ListOrder::default(),
        Some("desc") => ListOrder::Desc,
        Some("asc") => ListOrder::Asc,
        Some(value) => {
            return Err(AppError::BadRequest(format!(
                "'order' must be either 'desc' or 'asc', got '{value}'."
            )));
        }
    };

    let cursor = match cursor_str {
        Some(ref s) => {
            if sort != ListSort::CreatedAt {
                return Err(AppError::BadRequest(
                    "Cursor pagination is only supported with 'sort=createdAt'.".into(),
                ));
            }
            Some(Cursor::parse(s)?)
        }
        None => None,
    };

    Ok(ListQuery {
        pagination,
        cursor,
        sort,
        order,
        filter,
    })
}

/// 把一个 `hasId` / `missingId` 的值按逗号切开并并入 `set`
///
/// 逗号分隔（`hasId=ncmMusicId,isrc`）与重复传参（`hasId=ncmMusicId&hasId=isrc`）可以混用，
/// token 取并集。空 token 跳过，因此 `hasId=,` 与没传等价
fn collect_id_kinds(param: &str, value: &str, set: &mut IdKindSet) -> Result<(), AppError> {
    for token in value.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        let kind = ID_KIND_MAP
            .iter()
            .find(|(name, _)| *name == token)
            .map(|&(_, kind)| kind)
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "'{param}' must be one of {ID_KIND_VALUES}, got '{token}'."
                ))
            })?;
        set.insert(kind);
    }

    Ok(())
}

/// 按 [`ID_KIND_MAP`] 的顺序把集合渲染回对外取值名，用于报错消息
fn format_id_kinds(set: IdKindSet) -> String {
    ID_KIND_MAP
        .iter()
        .filter(|&&(_, kind)| set.contains(kind))
        .map(|(name, _)| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ")
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

    // --- 仅分页（列表端点）测试组 ---

    #[test]
    fn pagination_only_no_params_uses_defaults() {
        // 与 parse_search_query 的关键差异：列表端点不带查询串也是合法请求
        let pagination = parse_list_query("").unwrap().pagination;
        assert_eq!(pagination, Pagination::default());
        assert_eq!(pagination.page, 1);
        assert_eq!(pagination.page_size, 50);
    }

    #[test]
    fn pagination_only_page_and_page_size_parsed() {
        let pagination = parse_list_query("page=2&pageSize=10").unwrap().pagination;
        assert_eq!(pagination.page, 2);
        assert_eq!(pagination.page_size, 10);
    }

    #[test]
    fn pagination_only_empty_values_fall_back_to_defaults() {
        let pagination = parse_list_query("page=&pageSize=").unwrap().pagination;
        assert_eq!(pagination, Pagination::default());
    }

    #[test]
    fn pagination_only_search_params_are_ignored() {
        // 列表端点不接受检索条件，混进来的搜索参数既不生效也不报错
        let pagination = parse_list_query("musicName=ME!&q=hello&page=3")
            .unwrap()
            .pagination;
        assert_eq!(pagination.page, 3);
        assert_eq!(pagination.page_size, 50);
    }

    #[test]
    fn pagination_only_rejects_zero_page() {
        let result = parse_list_query("page=0");
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }

    #[test]
    fn pagination_only_rejects_non_numeric_page() {
        let result = parse_list_query("page=abc");
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }

    #[test]
    fn pagination_only_rejects_oversized_page_size() {
        let result = parse_list_query("pageSize=101");
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }

    #[test]
    fn list_query_defaults_sort_and_order() {
        let query = parse_list_query("").unwrap();
        assert_eq!(query.sort, ListSort::CreatedAt);
        assert_eq!(query.order, ListOrder::Desc);
    }

    #[test]
    fn list_query_parses_all_sort_values() {
        for (raw, expected) in [
            ("createdAt", ListSort::CreatedAt),
            ("id", ListSort::Id),
            ("musicName", ListSort::TrackName),
            ("artistName", ListSort::ArtistName),
            ("albumName", ListSort::AlbumName),
        ] {
            assert_eq!(
                parse_list_query(&format!("sort={raw}")).unwrap().sort,
                expected
            );
        }
    }

    #[test]
    fn list_query_rejects_invalid_sort_and_order() {
        assert!(matches!(
            parse_list_query("sort=created_at"),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            parse_list_query("order=down"),
            Err(AppError::BadRequest(_))
        ));
    }

    // --- 列表端点结构化过滤测试组 ---

    #[test]
    fn list_query_has_no_filter_by_default() {
        assert!(
            parse_list_query("page=2&sort=id")
                .unwrap()
                .filter
                .is_empty()
        );
    }

    #[test]
    fn list_query_parses_author_filters() {
        let filter = parse_list_query("authorId=12345&authorUsername=someone")
            .unwrap()
            .filter;
        assert_eq!(filter.author_id.as_deref(), Some("12345"));
        assert_eq!(filter.author_username.as_deref(), Some("someone"));
        assert!(filter.has.is_empty());
        assert!(filter.missing.is_empty());
    }

    #[test]
    fn list_query_keeps_author_values_verbatim() {
        // 与 /lyrics/search 的作者参数保持一致：不 trim、不改大小写，原样交给倒排索引比对
        let filter = parse_list_query("authorId=+Someone+&authorUsername=SomeOne")
            .unwrap()
            .filter;
        assert_eq!(filter.author_id.as_deref(), Some(" Someone "));
        assert_eq!(filter.author_username.as_deref(), Some("SomeOne"));
    }

    #[test]
    fn list_query_repeated_author_param_keeps_last() {
        let filter = parse_list_query("authorId=first&authorId=second")
            .unwrap()
            .filter;
        assert_eq!(filter.author_id.as_deref(), Some("second"));
    }

    #[test]
    fn list_query_parses_all_id_kind_values() {
        for &(raw, kind) in ID_KIND_MAP {
            let filter = parse_list_query(&format!("hasId={raw}")).unwrap().filter;
            assert!(filter.has.contains(kind), "hasId={raw} 未解析出 {kind:?}");
            assert!(filter.missing.is_empty());

            let filter = parse_list_query(&format!("missingId={raw}"))
                .unwrap()
                .filter;
            assert!(
                filter.missing.contains(kind),
                "missingId={raw} 未解析出 {kind:?}"
            );
            assert!(filter.has.is_empty());
        }
    }

    #[test]
    fn list_query_comma_and_repeated_id_forms_agree() {
        let comma = parse_list_query("hasId=ncmMusicId,isrc").unwrap().filter;
        let repeated = parse_list_query("hasId=ncmMusicId&hasId=isrc")
            .unwrap()
            .filter;
        let mixed = parse_list_query("hasId=ncmMusicId,isrc&hasId=isrc")
            .unwrap()
            .filter;

        assert_eq!(comma.has, repeated.has);
        assert_eq!(comma.has, mixed.has);
        assert!(comma.has.contains(IdKind::NcmMusicId));
        assert!(comma.has.contains(IdKind::Isrc));
        assert!(!comma.has.contains(IdKind::SpotifyId));
    }

    #[test]
    fn list_query_ignores_blank_id_filters() {
        for raw in ["hasId=", "hasId=+", "hasId=,", "missingId=,+,"] {
            let filter = parse_list_query(raw).unwrap().filter;
            assert!(filter.is_empty(), "{raw} 应当等同于没传");
        }
    }

    #[test]
    fn list_query_rejects_unknown_id_kind() {
        let err = parse_list_query("hasId=ncm").unwrap_err();
        let AppError::BadRequest(message) = err else {
            panic!("未知取值应当是 400");
        };
        assert!(message.contains("'hasId'"), "{message}");
        assert!(message.contains("got 'ncm'"), "{message}");

        assert!(matches!(
            parse_list_query("hasId=ncmMusicId,nope"),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            parse_list_query("missingId=platform"),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn list_query_rejects_overlapping_has_and_missing() {
        let err = parse_list_query("hasId=ncmMusicId,isrc&missingId=isrc,spotifyId").unwrap_err();
        let AppError::BadRequest(message) = err else {
            panic!("hasId 与 missingId 相交应当是 400");
        };
        // 只报真正相交的那个取值，另外两个各自只出现在一侧，不算冲突
        assert!(message.contains("'isrc'"), "{message}");
        assert!(!message.contains("'ncmMusicId'"), "{message}");
        assert!(!message.contains("'spotifyId'"), "{message}");
    }

    #[test]
    fn list_query_allows_disjoint_has_and_missing() {
        let filter = parse_list_query("hasId=ncmMusicId&missingId=isrc")
            .unwrap()
            .filter;
        assert!(filter.has.contains(IdKind::NcmMusicId));
        assert!(filter.missing.contains(IdKind::Isrc));
    }

    #[test]
    fn id_kind_message_values_match_map() {
        assert_eq!(ID_KIND_MAP.len(), IdKind::ALL.len());

        for kind in IdKind::ALL {
            let names: Vec<&str> = ID_KIND_MAP
                .iter()
                .filter(|&&(_, mapped)| mapped == kind)
                .map(|&(name, _)| name)
                .collect();
            assert_eq!(names.len(), 1, "{kind:?} 的对外取值应当唯一");
            assert!(
                ID_KIND_VALUES.contains(&format!("'{}'", names[0])),
                "报错消息的取值清单里缺少 {kind:?}"
            );
        }
    }

    // --- 列表端点游标与增量分页测试组 ---

    #[test]
    fn list_query_parses_cursor() {
        let query = parse_list_query("cursor=1768754400682_699269132670751").unwrap();
        let cursor = query.cursor.unwrap();
        assert_eq!(cursor.timestamp, 1_768_754_400_682);
        assert_eq!(cursor.id.get(), 699_269_132_670_751);
    }

    #[test]
    fn list_query_parses_before_as_cursor_alias() {
        let query = parse_list_query("before=1768754400682_699269132670751").unwrap();
        let cursor = query.cursor.unwrap();
        assert_eq!(cursor.timestamp, 1_768_754_400_682);
        assert_eq!(cursor.id.get(), 699_269_132_670_751);
    }

    #[test]
    fn list_query_allows_identical_cursor_and_before() {
        let query = parse_list_query(
            "cursor=1768754400682_699269132670751&before=1768754400682_699269132670751",
        )
        .unwrap();
        assert!(query.cursor.is_some());
    }

    #[test]
    fn list_query_rejects_conflicting_cursor_and_before() {
        let err = parse_list_query(
            "cursor=1768754400682_699269132670751&before=1768754400682_111111111111111",
        )
        .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn list_query_rejects_page_with_cursor() {
        let err = parse_list_query("page=2&cursor=1768754400682_699269132670751").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));

        let err = parse_list_query("page=1&before=1768754400682_699269132670751").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn list_query_rejects_cursor_with_non_created_at_sort() {
        let err = parse_list_query("sort=musicName&cursor=1768754400682_699269132670751").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));

        let err = parse_list_query("sort=id&cursor=1768754400682_699269132670751").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn list_query_rejects_malformed_cursor() {
        assert!(matches!(
            parse_list_query("cursor=abc"),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            parse_list_query("cursor=123_"),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn list_query_parses_since_and_until() {
        let query = parse_list_query("since=1000&until=2000").unwrap();
        assert_eq!(query.filter.since, Some(1000));
        assert_eq!(query.filter.until, Some(2000));
    }

    #[test]
    fn list_query_rejects_since_greater_than_until() {
        let err = parse_list_query("since=2000&until=1000").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn list_query_rejects_invalid_since_or_until() {
        assert!(matches!(
            parse_list_query("since=invalid"),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            parse_list_query("until=invalid"),
            Err(AppError::BadRequest(_))
        ));
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
