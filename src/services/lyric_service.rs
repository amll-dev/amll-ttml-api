use std::collections::HashMap;

use futures::StreamExt;

use crate::{
    api::{
        lrclib::dto::{
            LrclibSongItem,
            map_to_lrclib_item,
        },
        shared::dto::{
            ApiResponse,
            MatchContext,
            SearchData,
            SongItem,
            map_song_to_item,
        },
    },
    core::{
        error::AppError,
        models::{
            IdQuery,
            LyricHit,
            LyricIndexDB,
            LyricMatchField,
            SearchQuery,
            SongEntry,
        },
        repository::MetadataHit,
        state::AppState,
    },
    utils::matcher::MatchType,
};

pub struct SearchHit<'a> {
    pub entry: &'a SongEntry,
    pub metadata_score: MatchType,
    pub lyric_hit: Option<LyricHit>,
}

pub struct LyricService;

impl LyricService {
    const CONCURRENT_FETCH_LIMIT: usize = 10;

    pub async fn search_lyric(
        state: &AppState,
        query: &SearchQuery,
        limit: usize,
    ) -> ApiResponse<SearchData> {
        let db = state.db.load_full();
        let metadata_hits = db.search_by_fields(query);

        let lyric_hits =
            Self::fetch_lyric_hits_if_needed(state, query, &metadata_hits, limit).await;

        let sorted_hits =
            Self::merge_and_sort_hits(&db, metadata_hits, lyric_hits, query.lyric_text.is_some());

        let items: Vec<SongItem> = sorted_hits
            .into_iter()
            .take(limit)
            .map(|hit| {
                let match_context = hit.lyric_hit.and_then(|lyric| {
                    lyric.snippet.map(|snippet| MatchContext {
                        snippet: Some(snippet),
                    })
                });

                map_song_to_item(hit.entry, None, None, match_context)
            })
            .collect();

        ApiResponse {
            status: 200,
            data: SearchData { items },
        }
    }

    async fn fetch_lyric_hits_if_needed(
        state: &AppState,
        query: &SearchQuery,
        metadata_hits: &[MetadataHit<'_>],
        limit: usize,
    ) -> Vec<LyricHit> {
        // 计算仅凭元数据搜索的结果是否较弱，如果较低，或者结果较少，我们需要继续匹配歌词正文
        // 如果元数据已经足够精确了，并且用户没有明确要求去查歌词正文，那我们就不用继续匹配正文了
        let is_weak = metadata_hits.is_empty()
            || metadata_hits
                .first()
                .is_none_or(|h| h.score < MatchType::Medium)
            || metadata_hits.len() < limit;

        let fts_keyword = match (&query.lyric_text, &query.global_keyword) {
            (Some(explicit), _) => Some(explicit.clone()),
            (None, Some(global_q)) if is_weak => Some(global_q.clone()),
            _ => None,
        };

        if let Some(ref kw) = fts_keyword {
            match state.search_lyrics_fts(kw, limit as u64).await {
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

    fn merge_and_sort_hits<'a>(
        db: &'a LyricIndexDB,
        metadata_hits: Vec<MetadataHit<'a>>,
        lyric_hits: Vec<LyricHit>,
        is_explicit_lyric: bool,
    ) -> Vec<SearchHit<'a>> {
        let mut merged_map: HashMap<u64, SearchHit<'a>> = HashMap::new();

        if is_explicit_lyric {
            let is_full_metadata = metadata_hits.len() == db.entries.len();
            if is_full_metadata {
                for l_hit in lyric_hits {
                    if let Some(&idx) = db.id_idx.get(&l_hit.id) {
                        merged_map.insert(
                            l_hit.id,
                            SearchHit {
                                entry: &db.entries[idx],
                                metadata_score: MatchType::Perfect,
                                lyric_hit: Some(l_hit),
                            },
                        );
                    }
                }
            } else {
                let meta_map: HashMap<u64, MatchType> = metadata_hits
                    .into_iter()
                    .map(|h| (h.entry.id, h.score))
                    .collect();

                for l_hit in lyric_hits {
                    if let Some(&m_score) = meta_map.get(&l_hit.id)
                        && let Some(&idx) = db.id_idx.get(&l_hit.id)
                    {
                        merged_map.insert(
                            l_hit.id,
                            SearchHit {
                                entry: &db.entries[idx],
                                metadata_score: m_score,
                                lyric_hit: Some(l_hit),
                            },
                        );
                    }
                }
            }
        } else {
            for m_hit in metadata_hits {
                merged_map.insert(
                    m_hit.entry.id,
                    SearchHit {
                        entry: m_hit.entry,
                        metadata_score: m_hit.score,
                        lyric_hit: None,
                    },
                );
            }

            for l_hit in lyric_hits {
                if let Some(hit) = merged_map.get_mut(&l_hit.id) {
                    hit.lyric_hit = Some(l_hit);
                } else if let Some(&idx) = db.id_idx.get(&l_hit.id) {
                    merged_map.insert(
                        l_hit.id,
                        SearchHit {
                            entry: &db.entries[idx],
                            metadata_score: MatchType::NoMatch,
                            lyric_hit: Some(l_hit),
                        },
                    );
                }
            }
        }

        let mut sorted_hits: Vec<SearchHit<'a>> = merged_map.into_values().collect();

        let get_priority = |hit: &SearchHit| -> u8 {
            let has_lyric = hit.lyric_hit.is_some();
            let is_main = hit
                .lyric_hit
                .as_ref()
                .is_some_and(|l| l.field == LyricMatchField::MainLyric);
            let meta_strong = hit.metadata_score >= MatchType::Medium;
            let meta_exists = hit.metadata_score > MatchType::NoMatch;

            if meta_strong {
                return 5;
            } else if meta_exists && has_lyric {
                return 4;
            } else if is_main {
                return 3;
            } else if meta_exists {
                return 2;
            } else if has_lyric {
                return 1;
            }
            0
        };

        sorted_hits.sort_unstable_by(|a, b| {
            let p_a = get_priority(a);
            let p_b = get_priority(b);

            p_b.cmp(&p_a)
                .then_with(|| {
                    let r_a = a.lyric_hit.as_ref().map_or(1000.0, |l| l.rank);
                    let r_b = b.lyric_hit.as_ref().map_or(1000.0, |l| l.rank);
                    r_a.total_cmp(&r_b)
                })
                .then_with(|| b.metadata_score.cmp(&a.metadata_score))
                .then_with(|| b.entry.timestamp.cmp(&a.entry.timestamp))
        });

        sorted_hits
    }

    pub async fn get_lyric(
        state: &AppState,
        query: IdQuery,
        format: String,
    ) -> Result<ApiResponse<SongItem>, AppError> {
        let db = state.db.load();
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

        let ttml_text = state
            .fetch_lyric_ttml(latest_song_cloned.filename.as_str())
            .await?;

        let item = map_song_to_item(&latest_song_cloned, Some(ttml_text), Some(format), None);

        Ok(ApiResponse {
            status: 200,
            data: item,
        })
    }

    pub async fn lrclib_search(
        state: &AppState,
        query: &SearchQuery,
        limit: usize,
    ) -> Vec<LrclibSongItem> {
        let db = state.db.load();

        let matched_hits = db.search_by_fields(query);
        let entries: Vec<_> = matched_hits
            .into_iter()
            .take(limit)
            .map(|hit| hit.entry.clone())
            .collect();

        drop(db);

        futures::stream::iter(entries)
            .map(|entry| async move {
                let formatted = match state.fetch_parsed_lyric(entry.filename.as_str()).await {
                    Ok(f) => Some(f),
                    Err(e) => {
                        tracing::warn!(
                            "Failed to fetch parsed lyric for file '{}' in LRCLIB search: {e:?}",
                            entry.filename
                        );
                        None
                    }
                };
                map_to_lrclib_item(&entry, formatted.as_ref())
            })
            .buffered(Self::CONCURRENT_FETCH_LIMIT)
            .collect()
            .await
    }

    pub async fn lrclib_get_by_fields(
        state: &AppState,
        query: SearchQuery,
    ) -> Result<LrclibSongItem, AppError> {
        let db = state.db.load();
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

        let formatted = state
            .fetch_parsed_lyric(latest_song_cloned.filename.as_str())
            .await?;
        let item = map_to_lrclib_item(&latest_song_cloned, Some(&formatted));
        Ok(item)
    }

    pub async fn lrclib_get_by_id(state: &AppState, id: u64) -> Result<LrclibSongItem, AppError> {
        let db = state.db.load();

        let idx = db.id_idx.get(&id).copied().ok_or(AppError::LyricNotFound)?;
        let song_cloned = db.entries[idx].clone();
        drop(db);

        let formatted = state
            .fetch_parsed_lyric(song_cloned.filename.as_str())
            .await?;
        let item = map_to_lrclib_item(&song_cloned, Some(&formatted));
        Ok(item)
    }
}
