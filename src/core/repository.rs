use std::collections::HashMap;

use compact_str::CompactString;

use crate::{
    core::models::{
        IdQuery,
        LyricIndexDB,
        SearchQuery,
        SongEntry,
    },
    utils::string::contains_ignore_ascii_case,
};

impl LyricIndexDB {
    /// 用于 /api/get，严格 AND 交集。
    ///
    /// 必须严格匹配所有传入的 ID。只有某个歌词同时具有传入的所有 ID，才返回
    pub fn find_by_ids(&self, query: &IdQuery) -> Vec<usize> {
        let mut candidates: Option<Vec<usize>> = None;

        let mut apply_filter =
            |id_opt: &Option<String>, idx_map: &HashMap<CompactString, Vec<usize>>| {
                if let Some(id) = id_opt {
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

        apply_filter(&query.ncm_music_id, &self.ncm_idx);
        apply_filter(&query.qq_music_id, &self.qq_idx);
        apply_filter(&query.apple_music_id, &self.apple_idx);
        apply_filter(&query.spotify_id, &self.spotify_idx);
        apply_filter(&query.isrc, &self.isrc_idx);

        let mut result = candidates.unwrap_or_default();
        result.sort_unstable();
        result.dedup();
        result
    }

    /// 用于 /api/get，多字段或模糊搜索，支持字段交集
    pub fn search_by_fields(&self, query: &SearchQuery) -> Vec<&SongEntry> {
        let mut results = Vec::new();

        for entry in &self.entries {
            if let Some(ref q) = query.global_keyword {
                let match_track = entry
                    .track_names
                    .iter()
                    .any(|n| contains_ignore_ascii_case(n.as_str(), q));
                let match_artist = entry
                    .artist_names
                    .iter()
                    .any(|n| contains_ignore_ascii_case(n.as_str(), q));
                let match_album = entry
                    .album_names
                    .iter()
                    .any(|n| contains_ignore_ascii_case(n.as_str(), q));
                if !(match_track || match_artist || match_album) {
                    continue;
                }
            }

            // 具体字段的过滤为包含和 AND 关系
            if let Some(ref val) = query.track_name
                && !entry
                    .track_names
                    .iter()
                    .any(|n| contains_ignore_ascii_case(n.as_str(), val))
            {
                continue;
            }

            if let Some(ref val) = query.artist_name
                && !entry
                    .artist_names
                    .iter()
                    .any(|n| contains_ignore_ascii_case(n.as_str(), val))
            {
                continue;
            }

            if let Some(ref val) = query.album_name
                && !entry
                    .album_names
                    .iter()
                    .any(|n| contains_ignore_ascii_case(n.as_str(), val))
            {
                continue;
            }

            // ID 和用户名严格相等才算匹配
            if let Some(ref val) = query.author_id
                && !entry.author_ids.iter().any(|n| n.as_str() == val)
            {
                continue;
            }
            if let Some(ref val) = query.author_username
                && !entry.author_usernames.iter().any(|n| n.as_str() == val)
            {
                continue;
            }

            results.push(entry);
        }

        results
    }
}
