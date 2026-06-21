use std::collections::HashMap;

use compact_str::CompactString;

use crate::{
    core::models::{
        IdQuery,
        LyricIndexDB,
        SearchQuery,
        SongEntry,
    },
    utils::matcher::{
        MatchType,
        PreparedQuery,
        rough_match,
        score_entry,
    },
};

impl LyricIndexDB {
    /// 用于 /api/get，严格 AND 交集。
    ///
    /// 必须严格匹配所有传入的 ID。只有某个歌词同时具有传入的所有 ID，才返回
    pub fn find_by_ids(&self, query: &IdQuery) -> Vec<usize> {
        let mut candidates: Option<Vec<usize>> = None;

        let mut apply_filter = |ids: &[String], idx_map: &HashMap<CompactString, Vec<usize>>| {
            for id in ids {
                let matched = idx_map.get(id.as_str()).cloned().unwrap_or_default();

                candidates = match candidates.take() {
                    None => Some(matched),
                    Some(mut existing) => {
                        existing.retain(|x| matched.contains(x));
                        Some(existing)
                    }
                };
            }
        };

        apply_filter(&query.ncm_music_ids, &self.ncm_idx);
        apply_filter(&query.qq_music_ids, &self.qq_idx);
        apply_filter(&query.apple_music_ids, &self.apple_idx);
        apply_filter(&query.spotify_ids, &self.spotify_idx);
        apply_filter(&query.isrcs, &self.isrc_idx);

        let mut result = candidates.unwrap_or_default();
        result.sort_unstable();
        result.dedup();
        result
    }

    /// 用于 /api/get，多字段或模糊搜索，支持字段交集
    pub fn search_by_fields(&self, query: &SearchQuery) -> Vec<&SongEntry> {
        let prepared = PreparedQuery::from_search_query(query);

        let mut scored_results: Vec<(&SongEntry, MatchType)> = self
            .entries
            .iter()
            .filter(|entry| rough_match(&prepared, entry))
            .map(|entry| {
                let score = score_entry(query, entry);
                (entry, score)
            })
            .filter(|(_, score)| *score > MatchType::NoMatch)
            .collect();

        scored_results.sort_unstable_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.0.timestamp.cmp(&a.0.timestamp))
        });

        scored_results.into_iter().map(|(entry, _)| entry).collect()
    }
}
