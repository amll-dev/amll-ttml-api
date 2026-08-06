use std::collections::{
    HashMap,
    HashSet,
};

use compact_str::CompactString;

use crate::core::{
    matcher::{
        MatchType,
        PreparedQuery,
        rough_match,
        score_entry,
    },
    models::{
        IdQuery,
        LyricIndexDB,
        SearchQuery,
        SongEntry,
    },
};

#[derive(Debug, Clone)]
pub struct MetadataHit<'a> {
    pub entry: &'a SongEntry,
    pub score: MatchType,
}

impl LyricIndexDB {
    /// 用于 `/api/v1/lyrics/get` 接口，根据各种 ID 参数查找歌曲。
    ///
    /// 匹配优先级：
    /// 1. `id`：53 位 ID。如果传入了 `id`，将进行 O(1) 查找并直接返回，忽略其他所有参数。
    /// 2. `filename`：如果传入了 `filename`（且未传入
    ///    `id`），将精确匹配文件名并直接返回，忽略其他平台 ID。
    /// 3. 各个平台 ID：如果既未传入 `id` 也未传入 `filename`，则用 AND 交集严格匹配所有传入的平台
    ///    ID（如 `ncm_music_ids`、`spotify_ids`）。只有某个歌词同时具有传入的所有平台 ID
    ///    时，才返回。
    pub fn find_by_ids(&self, query: &IdQuery) -> Vec<usize> {
        if let Some(id) = query.id {
            return self
                .id_idx
                .get(&id)
                .map(|&idx| vec![idx])
                .unwrap_or_default();
        }

        if let Some(ref filename) = query.filename {
            return self
                .entries
                .iter()
                .enumerate()
                .find(|(_, entry)| entry.filename.as_str() == filename.as_str())
                .map(|(idx, _)| vec![idx])
                .unwrap_or_default();
        }

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

    /// 根据元数据字段（歌名、歌手、专辑及作者等）进行检索与模糊打分
    ///
    /// 若查询未包含任何具体元数据的约束（如只使用 `lyricText`
    /// 进行搜索），将返回所有候选条目并统一赋予 `MatchType::Perfect` 分数
    pub fn search_by_fields(&self, query: &SearchQuery) -> Vec<MetadataHit<'_>> {
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

        // 仅 author 过滤无文本字段时，候选集已是精确匹配结果，直接按时间戳降序返回
        if !prepared.has_text_fields() {
            let mut result: Vec<MetadataHit<'_>> = candidates
                .iter()
                .map(|&idx| MetadataHit {
                    entry: &self.entries[idx],
                    score: MatchType::Perfect,
                })
                .collect();
            result.sort_unstable_by_key(|h| std::cmp::Reverse(h.entry.timestamp));
            return result;
        }

        // 模糊打分排序
        let mut scored_results: Vec<MetadataHit<'_>> = candidates
            .iter()
            .filter_map(|&idx| {
                let entry = &self.entries[idx];
                if rough_match(&prepared, entry) {
                    let score = score_entry(&prepared, entry);
                    if score > MatchType::NoMatch {
                        Some(MetadataHit { entry, score })
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        scored_results.sort_unstable_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| b.entry.timestamp.cmp(&a.entry.timestamp))
        });

        scored_results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{
            matcher::normalize_name_for_comparison,
            models::SongEntry,
        },
        utils::id::generate_file_id,
    };

    #[allow(clippy::too_many_arguments)]
    fn make_song(
        filename: &str,
        timestamp: u64,
        track_names: &[&str],
        artist_names: &[&str],
        ncm_ids: &[&str],
        spotify_ids: &[&str],
        author_ids: &[&str],
        author_usernames: &[&str],
    ) -> SongEntry {
        SongEntry {
            id: generate_file_id(filename),
            filename: CompactString::new(filename),
            timestamp,
            track_names: track_names.iter().map(|s| CompactString::new(*s)).collect(),
            artist_names: artist_names
                .iter()
                .map(|s| CompactString::new(*s))
                .collect(),
            album_names: Box::default(),
            normalized_track_names: track_names
                .iter()
                .map(|s| CompactString::from(normalize_name_for_comparison(s)))
                .collect(),
            normalized_artist_names: artist_names
                .iter()
                .map(|s| CompactString::from(normalize_name_for_comparison(s)))
                .collect(),
            normalized_album_names: Box::default(),
            ncm_music_ids: ncm_ids.iter().map(|s| CompactString::new(*s)).collect(),
            qq_music_ids: Box::default(),
            apple_music_ids: Box::default(),
            spotify_ids: spotify_ids.iter().map(|s| CompactString::new(*s)).collect(),
            isrcs: Box::default(),
            author_ids: author_ids.iter().map(|s| CompactString::new(*s)).collect(),
            author_usernames: author_usernames
                .iter()
                .map(|s| CompactString::new(*s))
                .collect(),
        }
    }

    fn build_test_db(songs: Vec<SongEntry>) -> LyricIndexDB {
        let mut id_idx: HashMap<u64, usize> = HashMap::new();
        let mut ncm_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();
        let mut spotify_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();
        let mut author_id_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();
        let mut author_username_idx: HashMap<CompactString, Vec<usize>> = HashMap::new();

        for (i, song) in songs.iter().enumerate() {
            id_idx.insert(song.id, i);

            for id in &song.ncm_music_ids {
                ncm_idx.entry(id.clone()).or_default().push(i);
            }
            for id in &song.spotify_ids {
                spotify_idx.entry(id.clone()).or_default().push(i);
            }
            for id in &song.author_ids {
                author_id_idx.entry(id.clone()).or_default().push(i);
            }
            for id in &song.author_usernames {
                author_username_idx.entry(id.clone()).or_default().push(i);
            }
        }

        LyricIndexDB {
            entries: songs,
            id_idx,
            ncm_idx,
            qq_idx: HashMap::new(),
            apple_idx: HashMap::new(),
            spotify_idx,
            isrc_idx: HashMap::new(),
            author_id_idx,
            author_username_idx,
        }
    }

    // --- find_by_ids tests ---

    #[test]
    fn find_by_id_exact_match() {
        let songs = vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &["111"],
                &[],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song B"],
                &["Artist Y"],
                &["222"],
                &[],
                &[],
                &[],
            ),
        ];
        let target_id = songs[1].id;
        let db = build_test_db(songs);
        let query = IdQuery {
            id: Some(target_id),
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![1]);
    }

    #[test]
    fn find_by_id_not_found() {
        let db = build_test_db(vec![make_song(
            "a.ttml",
            100,
            &["Song A"],
            &["Artist X"],
            &["111"],
            &[],
            &[],
            &[],
        )]);
        let query = IdQuery {
            id: Some(999_999),
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert!(result.is_empty());
    }

    #[test]
    fn find_by_id_takes_priority_over_filename() {
        let songs = vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &[],
                &[],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song B"],
                &["Artist Y"],
                &[],
                &[],
                &[],
                &[],
            ),
        ];
        let id_of_b = songs[1].id;
        let db = build_test_db(songs);

        let query = IdQuery {
            filename: Some("a.ttml".into()),
            id: Some(id_of_b),
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![1]);
    }

    #[test]
    fn find_by_id_ignores_platform_ids_if_present_in_struct() {
        let songs = vec![make_song(
            "a.ttml",
            100,
            &["Song A"],
            &["Artist X"],
            &["111"],
            &[],
            &[],
            &[],
        )];
        let id = songs[0].id;
        let db = build_test_db(songs);
        let query = IdQuery {
            id: Some(id),
            ncm_music_ids: vec!["999".into()],
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn find_by_filename_exact_match() {
        let db = build_test_db(vec![
            make_song(
                "1768754400682-250306205-r6IrpmBd.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &["111"],
                &[],
                &[],
                &[],
            ),
            make_song(
                "1768754400683-250306205-r6IrpmBd.ttml",
                200,
                &["Song B"],
                &["Artist Y"],
                &["222"],
                &[],
                &[],
                &[],
            ),
        ]);
        let query = IdQuery {
            filename: Some("1768754400682-250306205-r6IrpmBd.ttml".into()),
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn find_by_filename_not_found() {
        let db = build_test_db(vec![make_song(
            "1768754400682-250306205-r6IrpmBd.ttml",
            100,
            &["Song A"],
            &["Artist X"],
            &["111"],
            &[],
            &[],
            &[],
        )]);
        let query = IdQuery {
            filename: Some("nonexistent.ttml".into()),
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert!(result.is_empty());
    }

    #[test]
    fn find_by_filename_ignores_other_ids() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &["111"],
                &[],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song B"],
                &["Artist Y"],
                &["222"],
                &[],
                &[],
                &[],
            ),
        ]);

        let query = IdQuery {
            filename: Some("b.ttml".into()),
            ncm_music_ids: vec!["111".into()],
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![1]);
    }

    #[test]
    fn find_by_single_ncm_id() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &["111"],
                &[],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song B"],
                &["Artist Y"],
                &["222"],
                &[],
                &[],
                &[],
            ),
        ]);
        let query = IdQuery {
            ncm_music_ids: vec!["111".into()],
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn find_by_single_spotify_id() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &[],
                &["spot1"],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song B"],
                &["Artist Y"],
                &[],
                &["spot2"],
                &[],
                &[],
            ),
        ]);
        let query = IdQuery {
            spotify_ids: vec!["spot2".into()],
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![1]);
    }

    #[test]
    fn find_by_cross_platform_and() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &["111"],
                &["spot1"],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song B"],
                &["Artist Y"],
                &["222"],
                &["spot2"],
                &[],
                &[],
            ),
        ]);
        // Entry 0 has ncm=111 & spotify=spot1
        let query = IdQuery {
            ncm_music_ids: vec!["111".into()],
            spotify_ids: vec!["spot1".into()],
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn find_by_cross_platform_and_no_match() {
        let db = build_test_db(vec![make_song(
            "a.ttml",
            100,
            &["Song A"],
            &["Artist X"],
            &["111"],
            &["spot1"],
            &[],
            &[],
        )]);
        // Entry 0 has ncm=111 but not spotify=spot2
        let query = IdQuery {
            ncm_music_ids: vec!["111".into()],
            spotify_ids: vec!["spot2".into()],
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert!(result.is_empty());
    }

    #[test]
    fn find_by_ids_multiple_same_type_and() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &["111", "222"],
                &[],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song B"],
                &["Artist Y"],
                &["222", "333"],
                &[],
                &[],
                &[],
            ),
        ]);
        // AND: need both 111 and 222 -> only entry 0
        let query = IdQuery {
            ncm_music_ids: vec!["111".into(), "222".into()],
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn find_by_ids_no_results() {
        let db = build_test_db(vec![make_song(
            "a.ttml",
            100,
            &["Song A"],
            &["Artist X"],
            &["111"],
            &[],
            &[],
            &[],
        )]);
        let query = IdQuery {
            ncm_music_ids: vec!["999".into()],
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert!(result.is_empty());
    }

    // --- search_by_fields tests ---

    #[test]
    fn search_by_music_name() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Love Story"],
                &["Taylor Swift"],
                &[],
                &[],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["ME!"],
                &["Taylor Swift"],
                &[],
                &[],
                &[],
                &[],
            ),
        ]);
        let query = SearchQuery {
            track_name: Some("Love Story".into()),
            ..Default::default()
        };
        let result = db.search_by_fields(&query);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entry.filename.as_str(), "a.ttml");
    }

    #[test]
    fn search_by_artist_name() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Taylor Swift"],
                &[],
                &[],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song B"],
                &["Ed Sheeran"],
                &[],
                &[],
                &[],
                &[],
            ),
        ]);
        let query = SearchQuery {
            artist_name: Some("Taylor".into()),
            ..Default::default()
        };
        let result = db.search_by_fields(&query);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entry.filename.as_str(), "a.ttml");
    }

    #[test]
    fn search_by_author_id_exact() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &[],
                &[],
                &["111"],
                &["user1"],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song A"],
                &["Artist X"],
                &[],
                &[],
                &["222"],
                &["user2"],
            ),
        ]);
        let query = SearchQuery {
            author_id: Some("111".into()),
            ..Default::default()
        };
        let result = db.search_by_fields(&query);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entry.filename.as_str(), "a.ttml");
    }

    #[test]
    fn search_by_author_id_and_music_name() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Love Story"],
                &["Taylor Swift"],
                &[],
                &[],
                &["111"],
                &["user1"],
            ),
            make_song(
                "b.ttml",
                200,
                &["ME!"],
                &["Taylor Swift"],
                &[],
                &[],
                &["111"],
                &["user1"],
            ),
            make_song(
                "c.ttml",
                300,
                &["Love Story"],
                &["Other Artist"],
                &[],
                &[],
                &["222"],
                &["user2"],
            ),
        ]);
        let query = SearchQuery {
            author_id: Some("111".into()),
            track_name: Some("Love Story".into()),
            ..Default::default()
        };
        let result = db.search_by_fields(&query);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entry.filename.as_str(), "a.ttml");
    }

    #[test]
    fn search_by_author_username() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Song A"],
                &["Artist X"],
                &[],
                &[],
                &["111"],
                &["apoint123"],
            ),
            make_song(
                "b.ttml",
                200,
                &["Song B"],
                &["Artist Y"],
                &[],
                &[],
                &["222"],
                &["other"],
            ),
        ]);
        let query = SearchQuery {
            author_username: Some("apoint123".into()),
            ..Default::default()
        };
        let result = db.search_by_fields(&query);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entry.filename.as_str(), "a.ttml");
    }

    #[test]
    fn search_by_global_keyword() {
        let db = build_test_db(vec![
            make_song(
                "a.ttml",
                100,
                &["Love Story"],
                &["Taylor Swift"],
                &[],
                &[],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                200,
                &["Shape of You"],
                &["Ed Sheeran"],
                &[],
                &[],
                &[],
                &[],
            ),
        ]);
        let query = SearchQuery {
            global_keyword: Some("Taylor".into()),
            ..Default::default()
        };
        let result = db.search_by_fields(&query);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].entry.filename.as_str(), "a.ttml");
    }

    #[test]
    fn search_no_results() {
        let db = build_test_db(vec![make_song(
            "a.ttml",
            100,
            &["Love Story"],
            &["Taylor Swift"],
            &[],
            &[],
            &[],
            &[],
        )]);
        let query = SearchQuery {
            track_name: Some("Completely Nonexistent Song Title".into()),
            ..Default::default()
        };
        let result = db.search_by_fields(&query);
        assert!(result.is_empty());
    }

    #[test]
    fn search_sorted_by_timestamp_desc_on_equal_score() {
        let db = build_test_db(vec![
            make_song(
                "old.ttml",
                100,
                &["ME!"],
                &["Taylor Swift"],
                &[],
                &[],
                &[],
                &[],
            ),
            make_song(
                "new.ttml",
                200,
                &["ME!"],
                &["Taylor Swift"],
                &[],
                &[],
                &[],
                &[],
            ),
        ]);
        let query = SearchQuery {
            track_name: Some("ME!".into()),
            ..Default::default()
        };
        let result = db.search_by_fields(&query);
        assert_eq!(result.len(), 2);
        // Newer first
        assert_eq!(result[0].entry.filename.as_str(), "new.ttml");
        assert_eq!(result[1].entry.filename.as_str(), "old.ttml");
    }
}
