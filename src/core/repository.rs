use std::collections::{
    HashMap,
    HashSet,
};

use compact_str::CompactString;

use crate::core::{
    LyricId,
    list_query::ListFilter,
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
            let derived_id = LyricId::from_filename(filename);
            return self
                .id_idx
                .get(&derived_id)
                .copied()
                .filter(|&idx| self.entries[idx].filename.as_str() == filename.as_str())
                .map(|idx| vec![idx])
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
            result.sort_unstable_by(|a, b| {
                b.entry
                    .timestamp
                    .cmp(&a.entry.timestamp)
                    .then_with(|| a.entry.id.cmp(&b.entry.id))
            });
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
                // id 全局唯一，兜底保证全序。lrclib_search 直接在这个顺序上翻页，
                // 少了它同分同时间戳的条目会在相邻页之间重复或漏掉。
                .then_with(|| a.entry.id.cmp(&b.entry.id))
        });

        scored_results
    }

    /// `/api/v1/lyrics/list` 的结构化过滤，返回命中条目在 [`Self::entries`] 里的下标
    ///
    /// 作者维度有倒排索引可用，先用它把候选集缩到该作者的投稿；`hasId` / `missingId`
    /// 的选择性太低（多数条目都有 ncm ID、都没有 ISRC），不值得为其建索引，
    /// 在候选集上逐条算掩码即可
    ///
    /// 返回顺序不作保证，排序由调用方负责
    #[must_use]
    pub fn list_candidates(&self, filter: &ListFilter) -> Vec<usize> {
        if filter.is_empty() {
            return (0..self.entries.len()).collect();
        }

        // 两个 `Option` 的 `Some` / `None` 只由「参数有没有传」决定，与索引查得到查不到无关：
        // 查不到的作者是空候选，而不是「该条件不生效」——否则两个作者参数同时传、
        // 其中一个查不到时，AND 会退化成只按另一个筛（`search_by_fields` 就有这个 bug）
        let by_author_id: Option<Vec<usize>> = filter.author_id.as_ref().map(|author_id| {
            self.author_id_idx
                .get(author_id.as_str())
                .cloned()
                .unwrap_or_default()
        });
        let by_author_username: Option<HashSet<usize>> =
            filter.author_username.as_ref().map(|author_username| {
                self.author_username_idx
                    .get(author_username.as_str())
                    .map(|indices| indices.iter().copied().collect())
                    .unwrap_or_default()
            });

        let candidates: Vec<usize> = match (by_author_id, by_author_username) {
            (Some(ids), Some(usernames)) => ids
                .into_iter()
                .filter(|idx| usernames.contains(idx))
                .collect(),
            (Some(ids), None) => ids,
            (None, Some(usernames)) => usernames.into_iter().collect(),
            (None, None) => (0..self.entries.len()).collect(),
        };

        candidates
            .into_iter()
            .filter(|&idx| {
                let entry = &self.entries[idx];
                if let Some(since) = filter.since
                    && entry.timestamp < since
                {
                    return false;
                }
                if let Some(until) = filter.until
                    && entry.timestamp > until
                {
                    return false;
                }
                let present = entry.present_id_kinds();
                present.contains_all(filter.has) && present.contains_none(filter.missing)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        LyricId,
        list_query::{
            IdKind,
            IdKindSet,
        },
        test_utils::make_song,
    };

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
        let db = LyricIndexDB::from_entries(songs);
        let query = IdQuery {
            id: Some(target_id),
            ..Default::default()
        };
        let result = db.find_by_ids(&query);
        assert_eq!(result, vec![1]);
    }

    #[test]
    fn find_by_id_not_found() {
        let db = LyricIndexDB::from_entries(vec![make_song(
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
            id: Some(LyricId::from_u64(999_999).unwrap()),
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
        let db = LyricIndexDB::from_entries(songs);

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
        let db = LyricIndexDB::from_entries(songs);
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![make_song(
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![make_song(
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![make_song(
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![
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
        let db = LyricIndexDB::from_entries(vec![make_song(
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
        let db = LyricIndexDB::from_entries(vec![
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

    #[test]
    fn author_only_search_breaks_ties_by_id() {
        // 同一批投稿的 timestamp 可能完全相同，此时必须由 id 兜底给出全序，
        // 否则 sort_unstable_by 的顺序不稳定，翻页会在相邻页之间重复或漏掉条目
        let entries: Vec<_> = ["a.ttml", "b.ttml", "c.ttml", "d.ttml"]
            .into_iter()
            .map(|filename| {
                make_song(
                    filename,
                    1_768_754_400_682,
                    &["ME!"],
                    &["Taylor Swift"],
                    &[],
                    &[],
                    &["108002475"],
                    &[],
                )
            })
            .collect();
        let db = LyricIndexDB::from_entries(entries);

        // 只按 author 过滤、无任何文本字段，走的是 has_text_fields() 为假的短路分支
        let query = SearchQuery {
            author_id: Some("108002475".into()),
            ..Default::default()
        };
        let result = db.search_by_fields(&query);

        assert_eq!(result.len(), 4);
        let ids: Vec<_> = result.iter().map(|h| h.entry.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "同时间戳条目应按 id 升序给出确定顺序");
    }

    // --- list_candidates tests ---

    /// 标识与作者两个维度都做成不对称的，任何一侧的判定写错都会有测试掉下来
    ///
    /// | 文件 | 携带的标识 | authorId | authorUsername |
    /// |---|---|---|---|
    /// | `all.ttml` | 全部五种 | `a1` | `alice` |
    /// | `ncm.ttml` | 仅 ncm | `a1` | `alice` |
    /// | `isrc.ttml` | 仅 isrc | `a2` | `bob` |
    /// | `bare.ttml` | 无 | `a2` | `alice` |
    fn filter_fixture() -> LyricIndexDB {
        LyricIndexDB::from_entries(vec![
            make_song(
                "all.ttml",
                400,
                &["All"],
                &["Artist"],
                &["111"],
                &["sp1"],
                &["a1"],
                &["alice"],
            )
            .with_qq_music_ids(&["qq1"])
            .with_apple_music_ids(&["ap1"])
            .with_isrcs(&["ISRC1"]),
            make_song(
                "ncm.ttml",
                300,
                &["Ncm"],
                &["Artist"],
                &["222"],
                &[],
                &["a1"],
                &["alice"],
            ),
            make_song(
                "isrc.ttml",
                200,
                &["Isrc"],
                &["Artist"],
                &[],
                &[],
                &["a2"],
                &["bob"],
            )
            .with_isrcs(&["ISRC2"]),
            make_song(
                "bare.ttml",
                100,
                &["Bare"],
                &["Artist"],
                &[],
                &[],
                &["a2"],
                &["alice"],
            ),
        ])
    }

    fn id_kinds(kinds: &[IdKind]) -> IdKindSet {
        let mut set = IdKindSet::empty();
        for &kind in kinds {
            set.insert(kind);
        }
        set
    }

    /// `list_candidates` 不保证顺序，断言时统一排序后比文件名
    fn matched_names(db: &LyricIndexDB, filter: &ListFilter) -> Vec<String> {
        let mut names: Vec<String> = db
            .list_candidates(filter)
            .into_iter()
            .map(|idx| db.entries[idx].filename.to_string())
            .collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn list_candidates_without_filter_returns_everything() {
        let db = filter_fixture();
        assert_eq!(
            matched_names(&db, &ListFilter::default()),
            ["all.ttml", "bare.ttml", "isrc.ttml", "ncm.ttml"]
        );
    }

    #[test]
    fn list_candidates_filters_by_author_id() {
        let db = filter_fixture();
        let filter = ListFilter {
            author_id: Some("a1".to_string()),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["all.ttml", "ncm.ttml"]);
    }

    #[test]
    fn list_candidates_intersects_both_author_dimensions() {
        let db = filter_fixture();
        let filter = ListFilter {
            author_id: Some("a2".to_string()),
            author_username: Some("alice".to_string()),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["bare.ttml"]);
    }

    #[test]
    fn list_candidates_unknown_author_yields_empty_not_unconstrained() {
        // 查不到的作者必须让整个 AND 为空，而不是让这个条件静默失效
        let db = filter_fixture();
        let filter = ListFilter {
            author_id: Some("nobody".to_string()),
            author_username: Some("alice".to_string()),
            ..ListFilter::default()
        };
        assert!(matched_names(&db, &filter).is_empty());

        let filter = ListFilter {
            author_id: Some("a1".to_string()),
            author_username: Some("nobody".to_string()),
            ..ListFilter::default()
        };
        assert!(matched_names(&db, &filter).is_empty());
    }

    #[test]
    fn list_candidates_requires_every_has_id() {
        let db = filter_fixture();
        let filter = ListFilter {
            has: id_kinds(&[IdKind::NcmMusicId]),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["all.ttml", "ncm.ttml"]);

        // 多个取值之间是 AND，只有同时携带两者的条目命中
        let filter = ListFilter {
            has: id_kinds(&[IdKind::NcmMusicId, IdKind::Isrc]),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["all.ttml"]);
    }

    #[test]
    fn list_candidates_rejects_any_missing_id() {
        let db = filter_fixture();
        let filter = ListFilter {
            missing: id_kinds(&[IdKind::Isrc]),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["bare.ttml", "ncm.ttml"]);

        let filter = ListFilter {
            missing: id_kinds(&[IdKind::Isrc, IdKind::NcmMusicId]),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["bare.ttml"]);
    }

    #[test]
    fn list_candidates_combines_has_and_missing() {
        let db = filter_fixture();
        let filter = ListFilter {
            has: id_kinds(&[IdKind::NcmMusicId]),
            missing: id_kinds(&[IdKind::Isrc]),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["ncm.ttml"]);
    }

    #[test]
    fn list_candidates_combines_author_and_id_kinds() {
        let db = filter_fixture();
        let filter = ListFilter {
            author_username: Some("alice".to_string()),
            missing: id_kinds(&[IdKind::NcmMusicId]),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["bare.ttml"]);
    }

    #[test]
    fn list_candidates_can_match_nothing() {
        // qq ID 只有 all.ttml 有，而它同时带着 ncm ID
        let db = filter_fixture();
        let filter = ListFilter {
            has: id_kinds(&[IdKind::QqMusicId]),
            missing: id_kinds(&[IdKind::NcmMusicId]),
            ..ListFilter::default()
        };
        assert!(matched_names(&db, &filter).is_empty());
    }

    #[test]
    fn list_candidates_filters_by_since() {
        let db = filter_fixture();
        let filter = ListFilter {
            since: Some(300),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["all.ttml", "ncm.ttml"]);
    }

    #[test]
    fn list_candidates_filters_by_until() {
        let db = filter_fixture();
        let filter = ListFilter {
            until: Some(200),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["bare.ttml", "isrc.ttml"]);
    }

    #[test]
    fn list_candidates_filters_by_since_and_until_window() {
        let db = filter_fixture();
        let filter = ListFilter {
            since: Some(200),
            until: Some(300),
            ..ListFilter::default()
        };
        assert_eq!(matched_names(&db, &filter), ["isrc.ttml", "ncm.ttml"]);
    }
}
