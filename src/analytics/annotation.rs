//! handler 侧的业务标注
//!
//! 中间件只看得到 HTTP 层，命中数、命中 ID 这类业务信息要由 handler 经 [`Annotator`] 交给它。
//! 统计功能关闭时 [`Annotator`] 是空壳，标注直接丢弃，handler 不需要关心开关

use std::{
    convert::Infallible,
    future::ready,
    sync::{
        Arc,
        Mutex,
        PoisonError,
    },
};

use axum::{
    extract::FromRequestParts,
    http::request::Parts,
};
use serde::Serialize;

use crate::{
    core::{
        LyricId,
        error::AppError,
        matcher::PreparedQuery,
        models::{
            IdQuery,
            LyricSearchResult,
            SearchQuery,
        },
    },
    utils::string::truncate_utf8,
};

/// 规范化查询里单个字段的长度上限（字节）
const MAX_NORM_FIELD_BYTES: usize = 256;

/// 结果是经由哪条路径找到的
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// 53 位歌词 ID
    Id,
    /// 文件名
    Filename,
    /// 平台 ID（网易云、QQ 音乐、Apple Music、Spotify、ISRC）
    PlatformId,
    /// 元数据模糊匹配
    Fuzzy,
    /// 歌词正文 FTS 检索
    Fts,
}

impl MatchKind {
    /// 落库的取值，SQL 手册按这些字符串筛选，改名等同于破坏已有数据
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Filename => "filename",
            Self::PlatformId => "platform_id",
            Self::Fuzzy => "fuzzy",
            Self::Fts => "fts",
        }
    }

    /// `/lyrics/get` 的查找方式，与 `LyricIndexDB::find_by_ids` 的优先级一致
    #[must_use]
    pub const fn for_id_query(query: &IdQuery) -> Self {
        if query.id.is_some() {
            Self::Id
        } else if query.filename.is_some() {
            Self::Filename
        } else {
            Self::PlatformId
        }
    }

    /// `/lyrics/search` 的一条结果：带正文命中的算 FTS，否则是元数据匹配
    #[must_use]
    pub const fn for_search_hit(hit: &LyricSearchResult) -> Self {
        if hit.lyric_hit.is_some() {
            Self::Fts
        } else {
            Self::Fuzzy
        }
    }
}

/// 一次请求的业务标注
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Annotation {
    /// 命中数：列表型端点取分页前的总数，单条查找取 0 或 1
    pub hit_count: Option<u64>,
    /// 命中的歌词 ID：单条查找取命中条目，列表型端点取本页第一条
    pub hit_id: Option<LyricId>,
    /// 单条查找记录查找方式（未命中也记），列表型端点记录本页第一条的来源
    pub match_kind: Option<MatchKind>,
    /// 未命中时的规范化查询（JSON 字符串）
    ///
    /// 与检索路径同一套繁简转换与名称归一化，「周杰倫」和「周杰伦」会落成同一个值。
    /// 这套逻辑在 `DuckDB` 里无法复现，只能在服务端算好写进去
    pub norm_query: Option<String>,
}

/// handler 用来提交业务标注的提取器
///
/// 统计中间件为每个请求插入一个启用的实例；中间件没挂上时提取到的是空壳
#[derive(Clone, Default)]
pub struct Annotator(Option<Arc<Mutex<Option<Annotation>>>>);

impl Annotator {
    pub(super) fn enabled() -> Self {
        Self(Some(Arc::default()))
    }

    pub(super) fn take(&self) -> Option<Annotation> {
        self.0
            .as_ref()
            .and_then(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take())
    }

    /// 统计关闭时不构造标注，省掉未命中时的繁简转换
    fn record(&self, build: impl FnOnce() -> Annotation) {
        if let Some(slot) = &self.0 {
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(build());
        }
    }

    /// 单条查找（`get` 系列）：命中记下条目 ID，`LyricNotFound` 记为未命中，其余错误不标注
    ///
    /// `query` 是文本条件，只在未命中时用于生成规范化查询；按 ID 查找传 `None`
    pub fn lookup<T>(
        &self,
        result: &Result<T, AppError>,
        id_of: impl FnOnce(&T) -> LyricId,
        kind: MatchKind,
        query: Option<&SearchQuery>,
    ) {
        let hit_id = match result {
            Ok(value) => Some(id_of(value)),
            Err(AppError::LyricNotFound) => None,
            Err(_) => return,
        };

        self.record(|| Annotation {
            hit_count: Some(u64::from(hit_id.is_some())),
            hit_id,
            match_kind: Some(kind),
            norm_query: hit_id
                .is_none()
                .then(|| query.and_then(normalized_query))
                .flatten(),
        });
    }

    /// 列表型端点（`search` 系列与 `list`）：`total` 为分页前的命中总数，`first` 为本页第一条
    ///
    /// `query` 是文本条件，只在总数为 0 时用于生成规范化查询
    pub fn listing(
        &self,
        total: u64,
        first: Option<(LyricId, Option<MatchKind>)>,
        query: Option<&SearchQuery>,
    ) {
        self.record(|| Annotation {
            hit_count: Some(total),
            hit_id: first.map(|(id, _)| id),
            match_kind: first.and_then(|(_, kind)| kind),
            norm_query: (total == 0)
                .then(|| query.and_then(normalized_query))
                .flatten(),
        });
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Annotator {
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        ready(Ok(parts
            .extensions
            .get::<Self>()
            .cloned()
            .unwrap_or_default()))
    }
}

#[derive(Serialize)]
struct NormalizedQuery<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    track: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    album: Option<&'a str>,
}

/// 把歌名、歌手、专辑与 `q` 规范化后序列化成 JSON，没有任何文本条件时返回 `None`
///
/// 作者与歌词正文条件不参与：它们描述的不是「想找哪首歌」
fn normalized_query(query: &SearchQuery) -> Option<String> {
    fn field(value: Option<&str>) -> Option<&str> {
        value.map(|s| truncate_utf8(s, MAX_NORM_FIELD_BYTES))
    }

    let prepared = PreparedQuery::from_search_query(query);
    if !prepared.has_text_fields() {
        return None;
    }

    serde_json::to_string(&NormalizedQuery {
        q: field(prepared.global_keyword.as_deref()),
        track: field(prepared.track_name.as_deref()),
        artist: field(prepared.artist_name.as_deref()),
        album: field(prepared.album_name.as_deref()),
    })
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> LyricId {
        LyricId::from_u64(n).unwrap()
    }

    fn track_query(track: &str, artist: &str) -> SearchQuery {
        SearchQuery {
            track_name: Some(track.to_string()),
            artist_name: Some(artist.to_string()),
            ..SearchQuery::default()
        }
    }

    #[test]
    fn disabled_annotator_drops_everything() {
        let annotator = Annotator::default();
        annotator.listing(3, Some((id(1), None)), None);
        assert_eq!(annotator.take(), None);
    }

    #[test]
    fn lookup_hit_records_id_and_kind() {
        let annotator = Annotator::enabled();
        let result: Result<LyricId, AppError> = Ok(id(42));
        annotator.lookup(&result, |&hit| hit, MatchKind::PlatformId, None);

        assert_eq!(
            annotator.take(),
            Some(Annotation {
                hit_count: Some(1),
                hit_id: Some(id(42)),
                match_kind: Some(MatchKind::PlatformId),
                norm_query: None,
            })
        );
    }

    #[test]
    fn lookup_miss_records_normalized_query() {
        let annotator = Annotator::enabled();
        let result: Result<LyricId, AppError> = Err(AppError::LyricNotFound);
        let query = track_query("晴天", "周杰倫");
        annotator.lookup(&result, |&hit| hit, MatchKind::Fuzzy, Some(&query));

        let annotation = annotator.take().unwrap();
        assert_eq!(annotation.hit_count, Some(0));
        assert_eq!(annotation.hit_id, None);
        assert_eq!(annotation.match_kind, Some(MatchKind::Fuzzy));
        // 繁体歌手名经 OpenCC 转成简体，与检索口径一致
        assert_eq!(
            annotation.norm_query.as_deref(),
            Some(r#"{"track":"晴天","artist":"周杰伦"}"#)
        );
    }

    #[test]
    fn lookup_ignores_server_errors() {
        let annotator = Annotator::enabled();
        let result: Result<LyricId, AppError> = Err(AppError::UpstreamError("down".into()));
        annotator.lookup(&result, |&hit| hit, MatchKind::Id, None);
        assert_eq!(annotator.take(), None);
    }

    #[test]
    fn listing_only_normalizes_on_zero_hits() {
        let query = track_query("Song", "Artist");

        let annotator = Annotator::enabled();
        annotator.listing(5, Some((id(7), Some(MatchKind::Fts))), Some(&query));
        let annotation = annotator.take().unwrap();
        assert_eq!(annotation.hit_count, Some(5));
        assert_eq!(annotation.hit_id, Some(id(7)));
        assert_eq!(annotation.match_kind, Some(MatchKind::Fts));
        assert_eq!(annotation.norm_query, None);

        let annotator = Annotator::enabled();
        annotator.listing(0, None, Some(&query));
        let annotation = annotator.take().unwrap();
        assert_eq!(annotation.hit_count, Some(0));
        assert_eq!(annotation.match_kind, None);
        assert_eq!(
            annotation.norm_query.as_deref(),
            Some(r#"{"track":"song","artist":"artist"}"#)
        );
    }

    #[test]
    fn normalized_query_skips_non_text_conditions() {
        let lyric_only = SearchQuery {
            lyric_text: Some("hello".to_string()),
            author_id: Some("123".to_string()),
            ..SearchQuery::default()
        };
        assert_eq!(normalized_query(&lyric_only), None);
    }

    #[test]
    fn normalized_query_truncates_long_fields() {
        let query = SearchQuery {
            global_keyword: Some("a".repeat(1000)),
            ..SearchQuery::default()
        };
        let json = normalized_query(&query).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["q"].as_str().unwrap().len(), MAX_NORM_FIELD_BYTES);
    }

    #[test]
    fn match_kind_follows_id_query_priority() {
        let mut query = IdQuery {
            ncm_music_ids: vec!["1".to_string()],
            ..IdQuery::default()
        };
        assert_eq!(MatchKind::for_id_query(&query), MatchKind::PlatformId);

        query.filename = Some("a.ttml".to_string());
        assert_eq!(MatchKind::for_id_query(&query), MatchKind::Filename);

        query.id = Some(id(1));
        assert_eq!(MatchKind::for_id_query(&query), MatchKind::Id);
    }
}
