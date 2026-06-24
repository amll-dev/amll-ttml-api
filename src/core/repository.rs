use std::collections::{
    HashMap,
    HashSet,
};

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

    /// 根据多个字段的组合条件搜索歌曲。
    ///
    /// 支持 `q` 全局关键词模糊匹配，`musicName`/`artistName`/`albumName` 模糊包含，
    /// 以及 `authorId`/`authorUsername` 严格全等匹配。多个参数之间为 AND 交集关系。
    /// 结果按匹配相关性降序排序，相关性相同时按时间戳降序。
    pub fn search_by_fields(&self, query: &SearchQuery) -> Vec<&SongEntry> {
        let prepared = PreparedQuery::from_search_query(query);

        // 如果传了歌词作者 ID 和用户名，直接精确匹配再模糊打分
        let candidates: Vec<usize> =
            if prepared.author_id.is_some() || prepared.author_username.is_some() {
                let author_id_set = prepared.author_id.as_ref().and_then(|id| {
                    self.author_id_idx
                        .get(id.as_str())
                        .map(|v| v.iter().copied().collect::<HashSet<usize>>())
                });
                let author_username_set = prepared.author_username.as_ref().and_then(|username| {
                    self.author_username_idx
                        .get(username.as_str())
                        .map(|v| v.iter().copied().collect::<HashSet<usize>>())
                });

                let candidate_set = match (author_id_set, author_username_set) {
                    (Some(a), Some(b)) => a.intersection(&b).copied().collect(),
                    (Some(a), None) => a,
                    (None, Some(b)) => b,
                    (None, None) => HashSet::new(),
                };

                candidate_set.into_iter().collect()
            } else {
                (0..self.entries.len()).collect()
            };

        // 模糊打分排序
        let mut scored_results: Vec<(&SongEntry, MatchType)> = candidates
            .iter()
            .filter_map(|&idx| {
                let entry = &self.entries[idx];
                if rough_match(&prepared, entry) {
                    let score = score_entry(&prepared, entry);
                    if score > MatchType::NoMatch {
                        Some((entry, score))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        scored_results.sort_unstable_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.0.timestamp.cmp(&a.0.timestamp))
        });

        scored_results.into_iter().map(|(entry, _)| entry).collect()
    }
}
