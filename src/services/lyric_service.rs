use futures::StreamExt;

use crate::{
    core::{
        LyricId,
        error::AppError,
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

pub async fn lrclib_search<R: LyricStore>(
    store: &R,
    query: &SearchQuery,
    pagination: Pagination,
) -> Vec<(SongEntry, Option<TTMLFormatResult>)> {
    let db = store.load_index().await;

    let matched_hits = db.search_by_fields(query);
    let paginated = paginate(matched_hits, pagination, |hit| hit.entry.clone());

    drop(db);

    futures::stream::iter(paginated.items)
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
        .await
}

pub async fn lrclib_get_by_fields<R: LyricStore>(
    store: &R,
    query: SearchQuery,
) -> Result<(SongEntry, TTMLFormatResult), AppError> {
    let db = store.load_index().await;
    let matched_hits = db.search_by_fields(&query);

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
            models::{
                LyricIndexDB,
                LyricMatchField,
            },
            test_utils::{
                build_test_db,
                make_song,
            },
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

        let db = build_test_db(vec![entry1, entry2]);
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

        let items = lrclib_search(&store, &query, default_pagination()).await;
        assert_eq!(items.len(), 2);
        for (entry, formatted) in &items {
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

        let (entry, formatted) = lrclib_get_by_fields(&store, query).await.unwrap();
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
        let err = lrclib_get_by_fields(&store, no_match_query).await;
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
