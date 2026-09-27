//! 歌词业务层，负责六个数据端点背后的业务操作
//!
//! - `search_lyric` —> `/lyrics/search`：元数据模糊匹配与歌词正文 FTS 检索，两路命中合并排序后分页
//! - `list_lyrics` —> `/lyrics/list`：不带检索条件的列表，支持结构化过滤，并按创建时间、ID、曲名、
//!   艺术家名或专辑名排序后分页
//! - `get_lyric` —> `/lyrics/get`：按 53 位 ID、文件名或平台 ID 定位歌曲，返回条目与歌词原文
//! - `lrclib_search` —> `/lrclib/search`：LRCLIB 兼容搜索，逐条附带解析后的歌词
//! - `lrclib_get_by_fields` / `lrclib_get_by_id` -> `/lrclib/get`：按字段组合或 ID
//!   取单条并附带解析后的歌词
//!
//! 本层只产出领域数据（歌曲条目与歌词内容），分页也在此完成；
//! JSON DTO 的映射与响应包装由 api 层的 handler 负责

use compact_str::CompactString;
use futures::StreamExt;

use crate::{
    core::{
        LyricId,
        error::AppError,
        list_query::{
            Cursor,
            ListOrder,
            ListQuery,
            ListSort,
        },
        matcher::MatchType,
        models::{
            IdQuery,
            LyricHit,
            LyricSearchResult,
            SearchQuery,
            SongEntry,
        },
        pagination::{
            Paginated,
            Pagination,
            PaginationInfo,
            paginate,
        },
        repository::MetadataHit,
    },
    services::{
        LyricStore,
        ranking::merge_and_sort_hits,
    },
    utils::ttml::TTMLFormatResult,
};

/// FTS 候选窗口相对页大小的冗余倍数
///
/// FTS 命中需要和元数据命中合并并去重，窗口里相当一部分条目会跟元数据重复，
/// 保留冗余才能保证合并后仍有足够候选供翻页
const FTS_WINDOW_FACTOR: u64 = 3;
/// FTS 候选窗口上限，防止深翻页把 SQL LIMIT 撑到不可控
const FTS_WINDOW_CAP: u64 = 500;
/// 元数据粗筛命中数量低于此阈值时，认为元数据命中所选偏弱，触发 FTS5 正文检索补全
const FTS_TRIGGER_MIN_HITS: usize = 10;
/// lrclib 搜索逐条拉取解析歌词的并发上限
const CONCURRENT_FETCH_LIMIT: usize = 10;

/// 按翻页深度动态放大 FTS 候选窗口，并限制到 `FTS_WINDOW_CAP`
fn fts_window(pagination: Pagination) -> u64 {
    pagination
        .page
        .saturating_mul(pagination.page_size)
        .saturating_mul(FTS_WINDOW_FACTOR)
        .min(FTS_WINDOW_CAP)
}

pub async fn search_lyric<R: LyricStore>(
    store: &R,
    query: &SearchQuery,
    pagination: Pagination,
) -> Paginated<LyricSearchResult> {
    let db = store.load_index().await;
    let metadata_hits = db.search_by_fields(query);

    let lyric_hits =
        fetch_lyric_hits_if_needed(store, query, &metadata_hits, fts_window(pagination)).await;

    let sorted_hits =
        merge_and_sort_hits(&db, metadata_hits, lyric_hits, query.lyric_text.is_some());

    paginate(sorted_hits, pagination, |hit| LyricSearchResult {
        entry: hit.entry.clone(),
        lyric_hit: hit.lyric_hit,
    })
}

/// 歌词列表，按请求指定的维度和方向排序后分页，可带结构化过滤
///
/// 过滤条件（作者、`hasId` / `missingId`）由 `LyricIndexDB::list_candidates` 判定，
/// 分页的 `total` 因此是过滤后的数量；条件全空时退化为全量列表
///
/// 排序键是 `SongEntry::timestamp`，即上游 CI 处理投稿时打的戳，不是合并进词库的时刻——
/// 上游 PR 可能拖数月才合并，因此本列表不等于「最近新增到词库」的顺序
///
/// 时间戳同值时用 `id` 兜底全序。timestamp 派生自上游文件名的毫秒前缀，同一批投稿
/// 完全可能同值，而 `sort_unstable_by` 不保证同键条目的相对顺序；少了兜底键，
/// 索引重建后相邻页之间会重复或漏掉条目
pub async fn list_lyrics<R: LyricStore>(store: &R, query: ListQuery) -> Paginated<SongEntry> {
    let db = store.load_index().await;

    let mut ordered: Vec<&SongEntry> = db
        .list_candidates(&query.filter)
        .into_iter()
        .map(|idx| &db.entries[idx])
        .collect();
    ordered.sort_unstable_by(|a, b| {
        let primary = match query.sort {
            ListSort::CreatedAt => a.timestamp.cmp(&b.timestamp),
            ListSort::Id => a.id.cmp(&b.id),
            ListSort::TrackName => first_name(&a.track_names).cmp(first_name(&b.track_names)),
            ListSort::ArtistName => first_name(&a.artist_names).cmp(first_name(&b.artist_names)),
            ListSort::AlbumName => first_name(&a.album_names).cmp(first_name(&b.album_names)),
        };
        let primary = match query.order {
            ListOrder::Desc => primary.reverse(),
            ListOrder::Asc => primary,
        };
        primary.then_with(|| a.id.cmp(&b.id))
    });

    let total = u64::try_from(ordered.len()).unwrap_or(u64::MAX);

    if let Some(cursor) = query.cursor {
        let start_idx = match query.order {
            ListOrder::Desc => ordered.partition_point(|entry| {
                entry.timestamp > cursor.timestamp
                    || (entry.timestamp == cursor.timestamp && entry.id.get() <= cursor.id.get())
            }),
            ListOrder::Asc => ordered.partition_point(|entry| {
                entry.timestamp < cursor.timestamp
                    || (entry.timestamp == cursor.timestamp && entry.id.get() <= cursor.id.get())
            }),
        };

        let page_size = usize::try_from(query.pagination.page_size).unwrap_or(usize::MAX);
        let end_idx = start_idx.saturating_add(page_size).min(ordered.len());
        let paged_items: Vec<SongEntry> = ordered[start_idx..end_idx]
            .iter()
            .map(|&entry| entry.clone())
            .collect();

        let has_more = end_idx < ordered.len();
        let next_cursor = if has_more {
            paged_items
                .last()
                .map(|last| Cursor::from_entry(last).to_compact_string())
        } else {
            None
        };

        Paginated {
            items: paged_items,
            pagination: PaginationInfo {
                page: None,
                page_size: query.pagination.page_size,
                total,
                total_pages: None,
                has_more,
                next_cursor,
            },
        }
    } else {
        let mut paginated = paginate(ordered, query.pagination, Clone::clone);
        if paginated.pagination.has_more {
            paginated.pagination.next_cursor = paginated
                .items
                .last()
                .map(|last| Cursor::from_entry(last).to_compact_string());
        }
        paginated
    }
}

fn first_name(names: &[CompactString]) -> &str {
    names.first().map_or("", |name| name.as_str())
}

async fn fetch_lyric_hits_if_needed<R: LyricStore>(
    store: &R,
    query: &SearchQuery,
    metadata_hits: &[MetadataHit<'_>],
    fts_window: u64,
) -> Vec<LyricHit> {
    // 计算仅凭元数据搜索的结果是否较弱，如果较低，或者结果较少，我们需要继续匹配歌词正文
    // 如果元数据已经足够精确了，并且用户没有明确要求去查歌词正文，那我们就不用继续匹配正文了
    let is_weak = metadata_hits.is_empty()
        || metadata_hits
            .first()
            .is_none_or(|h| h.score < MatchType::Medium)
        || metadata_hits.len() < FTS_TRIGGER_MIN_HITS;

    let fts_keyword = match (&query.lyric_text, &query.global_keyword) {
        (Some(explicit), _) => Some(explicit.clone()),
        (None, Some(global_q)) if is_weak => Some(global_q.clone()),
        _ => None,
    };

    if let Some(ref kw) = fts_keyword {
        match store.search_lyrics_fts(kw, fts_window).await {
            Ok(hits) => hits,
            Err(e) => {
                tracing::error!("SQLite FTS5 lyric search failed for keyword '{kw}': {e:?}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    }
}

pub async fn get_lyric<R: LyricStore>(
    store: &R,
    query: IdQuery,
) -> Result<(SongEntry, String), AppError> {
    let db = store.load_index().await;
    let matched_indices = db.find_by_ids(&query);

    if matched_indices.is_empty() {
        return Err(AppError::LyricNotFound);
    }

    let mut candidates: Vec<_> = matched_indices
        .into_iter()
        .map(|idx| &db.entries[idx])
        .collect();
    candidates.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

    let latest_song_cloned = candidates[0].clone();
    drop(db);

    let ttml_text = store
        .fetch_lyric_ttml(latest_song_cloned.filename.as_str())
        .await?;

    Ok((latest_song_cloned, ttml_text))
}

/// LRCLIB 兼容搜索，逐条附带解析后的歌词
///
/// 对外响应是裸数组，分页元数据只供请求统计取命中总数
pub async fn lrclib_search<R: LyricStore>(
    store: &R,
    query: &SearchQuery,
    pagination: Pagination,
) -> Paginated<(SongEntry, Option<TTMLFormatResult>)> {
    let db = store.load_index().await;

    let matched_hits = db.search_by_fields(query);
    let paginated = paginate(matched_hits, pagination, |hit| hit.entry.clone());

    drop(db);

    let items = futures::stream::iter(paginated.items)
        .map(|entry| async move {
            let formatted = match store.fetch_parsed_lyric(entry.filename.as_str()).await {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::warn!(
                        "Failed to fetch parsed lyric for file '{}' in LRCLIB search: {e:?}",
                        entry.filename
                    );
                    None
                }
            };
            (entry, formatted)
        })
        .buffered(CONCURRENT_FETCH_LIMIT)
        .collect()
        .await;

    Paginated {
        items,
        pagination: paginated.pagination,
    }
}

pub async fn lrclib_get_by_fields<R: LyricStore>(
    store: &R,
    query: &SearchQuery,
) -> Result<(SongEntry, TTMLFormatResult), AppError> {
    let db = store.load_index().await;
    let matched_hits = db.search_by_fields(query);

    if matched_hits.is_empty() {
        return Err(AppError::LyricNotFound);
    }

    let best_hit = &matched_hits[0];

    if best_hit.score < MatchType::Medium {
        return Err(AppError::LyricNotFound);
    }

    let latest_song_cloned = best_hit.entry.clone();
    drop(db);

    let formatted = store
        .fetch_parsed_lyric(latest_song_cloned.filename.as_str())
        .await?;
    Ok((latest_song_cloned, formatted))
}

pub async fn lrclib_get_by_id<R: LyricStore>(
    store: &R,
    id: LyricId,
) -> Result<(SongEntry, TTMLFormatResult), AppError> {
    let db = store.load_index().await;

    let idx = db.id_idx.get(&id).copied().ok_or(AppError::LyricNotFound)?;
    let song_cloned = db.entries[idx].clone();
    drop(db);

    let formatted = store
        .fetch_parsed_lyric(song_cloned.filename.as_str())
        .await?;
    Ok((song_cloned, formatted))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::Arc,
    };

    use super::*;
    use crate::{
        core::{
            list_query::{
                IdKind,
                IdKindSet,
                ListFilter,
            },
            models::{
                LyricIndexDB,
                LyricMatchField,
            },
            test_utils::make_song,
        },
        utils::ttml::parse_and_format_ttml,
    };

    #[derive(Default)]
    pub struct MemoryLyricStore {
        pub db: Arc<LyricIndexDB>,
        pub ttml_map: HashMap<String, String>,
        pub fts_results: HashMap<String, Vec<LyricHit>>,
    }

    #[allow(clippy::unused_async_trait_impl)]
    impl LyricStore for MemoryLyricStore {
        async fn fetch_lyric_ttml(&self, filename: &str) -> Result<String, AppError> {
            self.ttml_map
                .get(filename)
                .cloned()
                .ok_or(AppError::LyricNotFound)
        }

        async fn fetch_parsed_lyric(&self, filename: &str) -> Result<TTMLFormatResult, AppError> {
            let ttml = self.fetch_lyric_ttml(filename).await?;
            Ok(parse_and_format_ttml(&ttml))
        }

        async fn search_lyrics_fts(
            &self,
            keyword: &str,
            _limit: u64,
        ) -> Result<Vec<LyricHit>, AppError> {
            Ok(self.fts_results.get(keyword).cloned().unwrap_or_default())
        }

        async fn load_index(&self) -> Arc<LyricIndexDB> {
            Arc::clone(&self.db)
        }
    }

    fn sample_ttml() -> String {
        r#"<?xml version="1.0" encoding="utf-8"?>
<tt xmlns="http://www.w3.org/ns/ttml">
  <body>
    <div>
      <p begin="00:01.000" end="00:03.000">Hello World Lyric</p>
    </div>
  </body>
</tt>"#
            .to_string()
    }

    fn create_test_store() -> (MemoryLyricStore, LyricId, LyricId) {
        let entry1 = make_song(
            "test_song_one.ttml",
            1_600_000_000,
            &["Test Song One"],
            &["Artist Alpha"],
            &["1001"],
            &["sp1001"],
            &[],
            &[],
        );
        let id1 = entry1.id;

        let entry2 = make_song(
            "test_song_two.ttml",
            1_700_000_000,
            &["Test Song Two"],
            &["Artist Beta"],
            &["1002"],
            &["sp1002"],
            &[],
            &[],
        );
        let id2 = entry2.id;

        let db = LyricIndexDB::from_entries(vec![entry1, entry2]);
        let mut ttml_map = HashMap::new();
        ttml_map.insert("test_song_one.ttml".to_string(), sample_ttml());
        ttml_map.insert("test_song_two.ttml".to_string(), sample_ttml());

        let mut fts_results = HashMap::new();
        fts_results.insert(
            "Hello".to_string(),
            vec![LyricHit {
                id: id1,
                rank: 0.1,
                field: LyricMatchField::MainLyric,
                snippet: Some("Hello World Lyric".to_string()),
            }],
        );

        (
            MemoryLyricStore {
                db: Arc::new(db),
                ttml_map,
                fts_results,
            },
            id1,
            id2,
        )
    }

    fn default_pagination() -> Pagination {
        Pagination {
            page: 1,
            page_size: 10,
        }
    }

    fn default_list_query() -> ListQuery {
        ListQuery {
            pagination: default_pagination(),
            ..ListQuery::default()
        }
    }

    #[tokio::test]
    async fn list_lyrics_orders_newest_first() {
        let (store, id1, id2) = create_test_store();

        let res = list_lyrics(&store, default_list_query()).await;

        assert_eq!(res.items.len(), 2);
        // test_song_two 的 timestamp 更大，应排在前
        assert_eq!(res.items[0].id, id2);
        assert_eq!(res.items[1].id, id1);
        assert_eq!(res.pagination.total, 2);
        assert_eq!(res.pagination.total_pages, Some(1));
        assert!(!res.pagination.has_more);
    }

    #[tokio::test]
    async fn list_lyrics_paginates() {
        let (store, id1, id2) = create_test_store();
        let one_per_page = Pagination {
            page: 1,
            page_size: 1,
        };

        let first = list_lyrics(
            &store,
            ListQuery {
                pagination: one_per_page,
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].id, id2);
        assert_eq!(first.pagination.total, 2);
        assert_eq!(first.pagination.total_pages, Some(2));
        assert!(first.pagination.has_more);

        let second = list_lyrics(
            &store,
            ListQuery {
                pagination: Pagination {
                    page: 2,
                    page_size: 1,
                },
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].id, id1);
        assert!(!second.pagination.has_more);
    }

    #[tokio::test]
    async fn list_lyrics_page_past_end_is_empty_but_reports_total() {
        let (store, _, _) = create_test_store();

        let res = list_lyrics(
            &store,
            ListQuery {
                pagination: Pagination {
                    page: 99,
                    page_size: 10,
                },
                ..ListQuery::default()
            },
        )
        .await;

        assert!(res.items.is_empty());
        assert_eq!(res.pagination.total, 2);
        assert!(!res.pagination.has_more);
    }

    #[tokio::test]
    async fn list_lyrics_breaks_ties_by_id() {
        // 同一批投稿的 timestamp 可能完全相同，此时必须由 id 兜底给出确定的全序，
        // 否则翻页会在相邻页之间重复或漏掉条目
        let entries: Vec<_> = ["a.ttml", "b.ttml", "c.ttml"]
            .into_iter()
            .map(|filename| {
                make_song(
                    filename,
                    1_768_754_400_682,
                    &["Same Timestamp"],
                    &["Artist"],
                    &[],
                    &[],
                    &[],
                    &[],
                )
            })
            .collect();
        let store = MemoryLyricStore {
            db: Arc::new(LyricIndexDB::from_entries(entries)),
            ..Default::default()
        };

        let res = list_lyrics(&store, default_list_query()).await;

        assert_eq!(res.items.len(), 3);
        let ids: Vec<_> = res.items.iter().map(|entry| entry.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "同时间戳条目应按 id 升序给出确定顺序");
    }

    #[tokio::test]
    async fn list_lyrics_supports_artist_and_album_sort() {
        let (mut store, id1, id2) = create_test_store();

        store.db = Arc::new(LyricIndexDB::from_entries(
            store
                .db
                .entries
                .iter()
                .cloned()
                .map(|mut entry| {
                    if entry.id == id1 {
                        entry.artist_names = ["Zed Artist".into()].into();
                        entry.album_names = ["Alpha Album".into()].into();
                    } else {
                        entry.artist_names = ["Amy Artist".into()].into();
                        entry.album_names = ["Zulu Album".into()].into();
                    }
                    entry
                })
                .collect(),
        ));

        let artist_name_asc = list_lyrics(
            &store,
            ListQuery {
                sort: ListSort::ArtistName,
                order: ListOrder::Asc,
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(artist_name_asc.items[0].id, id2);

        let album_name_desc = list_lyrics(
            &store,
            ListQuery {
                sort: ListSort::AlbumName,
                order: ListOrder::Desc,
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(album_name_desc.items[0].id, id2);
    }

    #[tokio::test]
    async fn list_lyrics_supports_sort_and_order() {
        let (store, id1, id2) = create_test_store();

        let track_name_asc = list_lyrics(
            &store,
            ListQuery {
                sort: ListSort::TrackName,
                order: ListOrder::Asc,
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(
            track_name_asc
                .items
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![id1, id2]
        );

        let track_name_desc = list_lyrics(
            &store,
            ListQuery {
                sort: ListSort::TrackName,
                order: ListOrder::Desc,
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(
            track_name_desc
                .items
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![id2, id1]
        );

        let id_desc = list_lyrics(
            &store,
            ListQuery {
                sort: ListSort::Id,
                order: ListOrder::Desc,
                ..ListQuery::default()
            },
        )
        .await;
        let ids: Vec<_> = id_desc.items.iter().map(|entry| entry.id).collect();
        let mut expected = ids.clone();
        expected.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(ids, expected);

        let created_at_asc = list_lyrics(
            &store,
            ListQuery {
                sort: ListSort::CreatedAt,
                order: ListOrder::Asc,
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(
            created_at_asc
                .items
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![id1, id2]
        );
    }

    /// 三条歌词：`a1` / `alice` 有两条（其一带 ISRC），`a2` / `bob` 有一条（无 ncm 也无 ISRC）
    fn filter_test_store() -> MemoryLyricStore {
        let entries = vec![
            make_song(
                "with_isrc.ttml",
                300,
                &["Filter A"],
                &["Artist"],
                &["1001"],
                &[],
                &["a1"],
                &["alice"],
            )
            .with_isrcs(&["ISRC1"]),
            make_song(
                "no_isrc.ttml",
                200,
                &["Filter B"],
                &["Artist"],
                &["1002"],
                &[],
                &["a1"],
                &["alice"],
            ),
            make_song(
                "other_author.ttml",
                100,
                &["Filter C"],
                &["Artist"],
                &[],
                &[],
                &["a2"],
                &["bob"],
            ),
        ];

        MemoryLyricStore {
            db: Arc::new(LyricIndexDB::from_entries(entries)),
            ..Default::default()
        }
    }

    fn filtered_filenames(res: &Paginated<SongEntry>) -> Vec<&str> {
        res.items
            .iter()
            .map(|entry| entry.filename.as_str())
            .collect()
    }

    #[tokio::test]
    async fn list_lyrics_filter_shrinks_total() {
        let store = filter_test_store();

        let mut has_isrc = IdKindSet::empty();
        has_isrc.insert(IdKind::Isrc);

        let res = list_lyrics(
            &store,
            ListQuery {
                filter: ListFilter {
                    has: has_isrc,
                    ..ListFilter::default()
                },
                ..default_list_query()
            },
        )
        .await;

        assert_eq!(filtered_filenames(&res), ["with_isrc.ttml"]);
        // total 是过滤后的数量，不是索引里的条目总数
        assert_eq!(res.pagination.total, 1);
        assert_eq!(res.pagination.total_pages, Some(1));
        assert!(!res.pagination.has_more);
    }

    #[tokio::test]
    async fn list_lyrics_filter_respects_sort_and_order() {
        let store = filter_test_store();
        let filter = ListFilter {
            author_username: Some("alice".to_string()),
            ..ListFilter::default()
        };

        let desc = list_lyrics(
            &store,
            ListQuery {
                filter: filter.clone(),
                ..default_list_query()
            },
        )
        .await;
        assert_eq!(
            filtered_filenames(&desc),
            ["with_isrc.ttml", "no_isrc.ttml"]
        );

        let asc = list_lyrics(
            &store,
            ListQuery {
                order: ListOrder::Asc,
                filter,
                ..default_list_query()
            },
        )
        .await;
        assert_eq!(filtered_filenames(&asc), ["no_isrc.ttml", "with_isrc.ttml"]);
    }

    #[tokio::test]
    async fn list_lyrics_filter_without_matches_is_empty_not_an_error() {
        let store = filter_test_store();

        let mut has_ncm = IdKindSet::empty();
        has_ncm.insert(IdKind::NcmMusicId);

        let res = list_lyrics(
            &store,
            ListQuery {
                filter: ListFilter {
                    author_id: Some("a2".to_string()),
                    has: has_ncm,
                    ..ListFilter::default()
                },
                ..default_list_query()
            },
        )
        .await;

        assert!(res.items.is_empty());
        assert_eq!(res.pagination.total, 0);
        assert_eq!(res.pagination.total_pages, Some(0));
        assert!(!res.pagination.has_more);
    }

    #[tokio::test]
    async fn list_lyrics_cursor_walks_across_pages() {
        use crate::core::list_query::Cursor;

        let (store, id1, id2) = create_test_store();

        // 第 1 页：offset 模式 pageSize=1，应带回 id2 与 nextCursor
        let first = list_lyrics(
            &store,
            ListQuery {
                pagination: Pagination {
                    page: 1,
                    page_size: 1,
                },
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].id, id2);
        assert!(first.pagination.has_more);
        let cursor_str = first.pagination.next_cursor.expect("应当携带 next_cursor");

        // 第 2 页：游标模式传入第一页末尾游标，应带回 id1，且无更多数据
        let second = list_lyrics(
            &store,
            ListQuery {
                pagination: Pagination {
                    page: 1,
                    page_size: 1,
                },
                cursor: Some(Cursor::parse(&cursor_str).unwrap()),
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].id, id1);
        assert_eq!(second.pagination.page, None);
        assert_eq!(second.pagination.total_pages, None);
        assert_eq!(second.pagination.total, 2);
        assert!(!second.pagination.has_more);
        assert!(second.pagination.next_cursor.is_none());

        // 第 3 页：使用第 2 页末尾的标识再次查询，应为空页
        let third_cursor = format!("{}_{}", second.items[0].timestamp, id1.get());
        let third = list_lyrics(
            &store,
            ListQuery {
                pagination: Pagination {
                    page: 1,
                    page_size: 1,
                },
                cursor: Some(Cursor::parse(&third_cursor).unwrap()),
                ..ListQuery::default()
            },
        )
        .await;
        assert!(third.items.is_empty());
        assert!(!third.pagination.has_more);
    }

    #[tokio::test]
    async fn list_lyrics_cursor_supports_asc_order() {
        use crate::core::list_query::Cursor;

        let (store, id1, id2) = create_test_store();

        // Asc 排序下，id1（较旧）在前，id2（较新）在后
        let first = list_lyrics(
            &store,
            ListQuery {
                pagination: Pagination {
                    page: 1,
                    page_size: 1,
                },
                order: ListOrder::Asc,
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(first.items[0].id, id1);
        assert!(first.pagination.has_more);
        let cursor_str = first.pagination.next_cursor.unwrap();

        let second = list_lyrics(
            &store,
            ListQuery {
                pagination: Pagination {
                    page: 1,
                    page_size: 1,
                },
                order: ListOrder::Asc,
                cursor: Some(Cursor::parse(&cursor_str).unwrap()),
                ..ListQuery::default()
            },
        )
        .await;
        assert_eq!(second.items[0].id, id2);
        assert!(!second.pagination.has_more);
    }

    #[tokio::test]
    async fn list_lyrics_since_and_until_filters() {
        let store = filter_test_store();

        // 筛选 since=200：命中 300(with_isrc) 和 200(no_isrc)
        let res_since = list_lyrics(
            &store,
            ListQuery {
                filter: ListFilter {
                    since: Some(200),
                    ..ListFilter::default()
                },
                ..default_list_query()
            },
        )
        .await;
        assert_eq!(
            filtered_filenames(&res_since),
            ["with_isrc.ttml", "no_isrc.ttml"]
        );
        assert_eq!(res_since.pagination.total, 2);

        // 筛选 until=200：命中 200(no_isrc) 和 100(other_author)
        let res_until = list_lyrics(
            &store,
            ListQuery {
                filter: ListFilter {
                    until: Some(200),
                    ..ListFilter::default()
                },
                ..default_list_query()
            },
        )
        .await;
        assert_eq!(
            filtered_filenames(&res_until),
            ["no_isrc.ttml", "other_author.ttml"]
        );
        assert_eq!(res_until.pagination.total, 2);

        // 筛选 since=200, until=200 时间窗口：仅命中 200(no_isrc)
        let res_window = list_lyrics(
            &store,
            ListQuery {
                filter: ListFilter {
                    since: Some(200),
                    until: Some(200),
                    ..ListFilter::default()
                },
                ..default_list_query()
            },
        )
        .await;
        assert_eq!(filtered_filenames(&res_window), ["no_isrc.ttml"]);
        assert_eq!(res_window.pagination.total, 1);
    }

    #[tokio::test]
    async fn search_lyric_metadata_hit() {
        let (store, id1, _) = create_test_store();
        let query = SearchQuery {
            track_name: Some("Test Song One".to_string()),
            ..Default::default()
        };

        let res = search_lyric(&store, &query, default_pagination()).await;
        assert_eq!(res.items.len(), 1);
        assert_eq!(res.items[0].entry.id, id1);
        assert_eq!(res.items[0].entry.filename.as_str(), "test_song_one.ttml");
        assert!(res.items[0].lyric_hit.is_none());
        assert_eq!(res.pagination.total, 1);
        assert!(!res.pagination.has_more);
    }

    #[tokio::test]
    async fn search_lyric_fts_hit_carries_lyric_hit() {
        let (store, id1, _) = create_test_store();
        let query = SearchQuery {
            lyric_text: Some("Hello".to_string()),
            ..Default::default()
        };

        let res = search_lyric(&store, &query, default_pagination()).await;
        assert_eq!(res.items.len(), 1);
        assert_eq!(res.items[0].entry.id, id1);

        let lyric_hit = res.items[0].lyric_hit.as_ref().unwrap();
        assert_eq!(lyric_hit.field, LyricMatchField::MainLyric);
        assert_eq!(lyric_hit.snippet.as_deref(), Some("Hello World Lyric"));
    }

    #[tokio::test]
    async fn get_lyric_returns_entry_and_ttml() {
        let (store, id1, _) = create_test_store();
        let query = IdQuery {
            spotify_ids: vec!["sp1001".to_string()],
            ..Default::default()
        };

        let (entry, ttml_text) = get_lyric(&store, query).await.unwrap();
        assert_eq!(entry.id, id1);
        assert_eq!(ttml_text, sample_ttml());

        let invalid_query = IdQuery {
            spotify_ids: vec!["non_existent".to_string()],
            ..Default::default()
        };
        let err = get_lyric(&store, invalid_query).await;
        assert!(matches!(err, Err(AppError::LyricNotFound)));
    }

    #[tokio::test]
    async fn lrclib_search_returns_entries_with_parsed_lyrics() {
        let (store, _, _) = create_test_store();
        let query = SearchQuery {
            global_keyword: Some("Artist".to_string()),
            ..Default::default()
        };

        let res = lrclib_search(&store, &query, default_pagination()).await;
        assert_eq!(res.items.len(), 2);
        assert_eq!(res.pagination.total, 2);
        for (entry, formatted) in &res.items {
            assert!(!entry.filename.as_str().is_empty());
            let formatted = formatted.as_ref().unwrap();
            assert_eq!(formatted.plain_lyrics.as_deref(), Some("Hello World Lyric"));
            assert_eq!(
                formatted.synced_lyrics.as_deref(),
                Some("[00:01.00] Hello World Lyric")
            );
        }
    }

    #[tokio::test]
    async fn lrclib_get_by_fields_returns_entry_and_parsed_lyric() {
        let (store, id1, _) = create_test_store();
        let query = SearchQuery {
            track_name: Some("Test Song One".to_string()),
            artist_name: Some("Artist Alpha".to_string()),
            ..Default::default()
        };

        let (entry, formatted) = lrclib_get_by_fields(&store, &query).await.unwrap();
        assert_eq!(entry.id, id1);
        assert_eq!(entry.track_names[0].as_str(), "Test Song One");
        assert_eq!(formatted.plain_lyrics.as_deref(), Some("Hello World Lyric"));
        assert_eq!(
            formatted.synced_lyrics.as_deref(),
            Some("[00:01.00] Hello World Lyric")
        );

        let no_match_query = SearchQuery {
            track_name: Some("NonExistentTrack".to_string()),
            ..Default::default()
        };
        let err = lrclib_get_by_fields(&store, &no_match_query).await;
        assert!(matches!(err, Err(AppError::LyricNotFound)));
    }

    #[tokio::test]
    async fn lrclib_get_by_id_returns_entry_and_parsed_lyric() {
        let (store, _, id2) = create_test_store();
        let (entry, formatted) = lrclib_get_by_id(&store, id2).await.unwrap();
        assert_eq!(entry.id, id2);
        assert!(formatted.synced_lyrics.is_some());

        let err = lrclib_get_by_id(&store, LyricId::from_u64(9999).unwrap()).await;
        assert!(matches!(err, Err(AppError::LyricNotFound)));
    }
}
