use std::collections::HashMap;

use crate::core::{
    LyricId,
    matcher::MatchType,
    models::{
        LyricHit,
        LyricIndexDB,
        LyricMatchField,
        SongEntry,
    },
    repository::MetadataHit,
};

pub struct SearchHit<'a> {
    pub entry: &'a SongEntry,
    pub metadata_score: MatchType,
    pub lyric_hit: Option<LyricHit>,
}

pub fn merge_and_sort_hits<'a>(
    db: &'a LyricIndexDB,
    metadata_hits: Vec<MetadataHit<'a>>,
    lyric_hits: Vec<LyricHit>,
    is_explicit_lyric: bool,
) -> Vec<SearchHit<'a>> {
    let mut merged_map: HashMap<LyricId, SearchHit<'a>> = HashMap::new();

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
            let meta_map: HashMap<LyricId, MatchType> = metadata_hits
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

    // 计算命中标的的 5 级优先级桶:
    // - Level 5 (元数据强命中): `metadata_score` >= `MatchType::Medium`
    //   歌名/歌手/专辑精准或高匹配，置信度最高，优先排在首位
    // - Level 4 (元数据弱命中 + 歌词命中): `metadata_score` > `MatchType::NoMatch` && `has_lyric`
    //   元数据有弱模糊匹配且歌词正文同时也命中，两路互相印证
    // - Level 3 (主歌词正文命中): `lyric_hit.field` == `LyricMatchField::MainLyric`
    //   凭借主歌词正文全文检索命中，权重高于伴唱/和声
    // - Level 2 (仅元数据弱命中): `metadata_score` > `MatchType::NoMatch`
    //   仅元数据有弱模糊匹配，且无歌词正文命中
    // - Level 1 (仅背景人声歌词命中): `has_lyric` (`field` == `LyricMatchField::BackgroundVocal`)
    //   仅在背景人声歌词中检索到匹配
    // - Level 0 (无匹配兜底): 兜底其他情况
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
            // 候选来自 HashMap，迭代顺序不确定；id 全局唯一，补上它让比较成为
            // 全序，否则同分同时间戳的条目会在相邻页之间重复或漏掉
            .then_with(|| a.entry.id.cmp(&b.entry.id))
    });

    sorted_hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_utils::make_song;

    // 维度 1：优先级分桶与排序压制关系测试
    // 测试不同等级的匹配类型（如元数据强命中、主歌词正文命中、
    // 背景歌词命中）之间是否符合预期的优先级分桶压制规则
    #[test]
    fn test_priority_tiering() {
        let e1 = make_song(
            "song1.ttml",
            1_000,
            &["Track 1"],
            &["Artist 1"],
            &[],
            &[],
            &[],
            &[],
        );
        let e2 = make_song(
            "song2.ttml",
            1_000,
            &["Track 2"],
            &["Artist 2"],
            &[],
            &[],
            &[],
            &[],
        );
        let e3 = make_song(
            "song3.ttml",
            1_000,
            &["Track 3"],
            &["Artist 3"],
            &[],
            &[],
            &[],
            &[],
        );

        let db = LyricIndexDB::from_entries(vec![e1.clone(), e2.clone(), e3.clone()]);

        // e1: Priority 5 (Metadata Medium)
        // e2: Priority 3 (Main Lyric match, no metadata match)
        // e3: Priority 1 (Background Vocal match, no metadata match)
        let metadata_hits = vec![MetadataHit {
            entry: &db.entries[0],
            score: MatchType::Medium,
        }];

        let lyric_hits = vec![
            LyricHit {
                id: db.entries[1].id,
                rank: 0.1,
                field: LyricMatchField::MainLyric,
                snippet: None,
            },
            LyricHit {
                id: db.entries[2].id,
                rank: 0.1,
                field: LyricMatchField::BackgroundVocal,
                snippet: None,
            },
        ];

        let results = merge_and_sort_hits(&db, metadata_hits, lyric_hits, false);

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].entry.id, e1.id); // Priority 5
        assert_eq!(results[1].entry.id, e2.id); // Priority 3
        assert_eq!(results[2].entry.id, e3.id); // Priority 1
    }

    // 维度 2：Break-tie 多级断平规则测试 (BM25, MatchType, Timestamp)
    // 当两条记录处于同一 Priority
    // 桶内时，测试算法能否按顺序正确使用二级、三级、四级指标解决“平局”
    #[test]
    fn test_break_tie_mechanics() {
        let e1 = make_song(
            "song1.ttml",
            1_000,
            &["Track 1"],
            &["Artist 1"],
            &[],
            &[],
            &[],
            &[],
        );
        let e2 = make_song(
            "song2.ttml",
            2_000,
            &["Track 2"],
            &["Artist 2"],
            &[],
            &[],
            &[],
            &[],
        );

        let db = LyricIndexDB::from_entries(vec![e1.clone(), e2.clone()]);

        // 场景 A：比较 BM25 rank (0.1 比 0.5 优先)
        let lyric_hits_bm25 = vec![
            LyricHit {
                id: db.entries[0].id,
                rank: 0.5,
                field: LyricMatchField::MainLyric,
                snippet: None,
            },
            LyricHit {
                id: db.entries[1].id,
                rank: 0.1,
                field: LyricMatchField::MainLyric,
                snippet: None,
            },
        ];

        let res_bm25 = merge_and_sort_hits(&db, vec![], lyric_hits_bm25, false);
        assert_eq!(res_bm25[0].entry.id, e2.id);
        assert_eq!(res_bm25[1].entry.id, e1.id);

        // 场景 B：BM25 相同，比较元数据 MatchType (High 比 Medium 优先)
        let meta_hits = vec![
            MetadataHit {
                entry: &db.entries[0],
                score: MatchType::High,
            },
            MetadataHit {
                entry: &db.entries[1],
                score: MatchType::Medium,
            },
        ];

        let res_meta = merge_and_sort_hits(&db, meta_hits, vec![], false);
        assert_eq!(res_meta[0].entry.id, e1.id);
        assert_eq!(res_meta[1].entry.id, e2.id);

        // 场景 C：得分、BM25 全相同，比较 timestamp (2000 比 1000 优先)
        let meta_hits_same_score = vec![
            MetadataHit {
                entry: &db.entries[0],
                score: MatchType::Medium,
            },
            MetadataHit {
                entry: &db.entries[1],
                score: MatchType::Medium,
            },
        ];

        let res_ts = merge_and_sort_hits(&db, meta_hits_same_score, vec![], false);
        assert_eq!(res_ts[0].entry.id, e2.id);
        assert_eq!(res_ts[1].entry.id, e1.id);
    }

    // 维度 3：严格全序与 ID 确定性保底测试
    // 测试当所有业务指标（Priority、BM25、元数据分、时间戳）完全相同时，排序是否具有 100%
    // 确定性的绝对全序
    #[test]
    fn test_strict_total_order_determinism() {
        let e1 = make_song("a.ttml", 1_000, &["Track"], &["Artist"], &[], &[], &[], &[]);
        let e2 = make_song("b.ttml", 1_000, &["Track"], &["Artist"], &[], &[], &[], &[]);

        let (first, second) = if e1.id < e2.id { (e1, e2) } else { (e2, e1) };

        let db = LyricIndexDB::from_entries(vec![first.clone(), second.clone()]);

        let meta_hits = vec![
            MetadataHit {
                entry: &db.entries[0],
                score: MatchType::Medium,
            },
            MetadataHit {
                entry: &db.entries[1],
                score: MatchType::Medium,
            },
        ];

        // 运行多次，确保输出结果顺序 100% 确定，总是 id 小的在前
        for _ in 0..5 {
            let res = merge_and_sort_hits(&db, meta_hits.clone(), vec![], false);
            assert_eq!(res[0].entry.id, first.id);
            assert_eq!(res[1].entry.id, second.id);
        }
    }

    // 维度 4：显式歌词搜索 vs 隐式搜索合并策略差异测试
    // 测试用户传入 lyricText（显式歌词搜索）与仅传 q /
    // 元数据字段（隐式搜索）时，结果集的集合合并与裁剪策略
    #[test]
    fn test_explicit_vs_implicit_search() {
        let e1 = make_song(
            "song1.ttml",
            1_000,
            &["Track 1"],
            &["Artist 1"],
            &[],
            &[],
            &[],
            &[],
        );
        let e2 = make_song(
            "song2.ttml",
            1_000,
            &["Track 2"],
            &["Artist 2"],
            &[],
            &[],
            &[],
            &[],
        );

        let db = LyricIndexDB::from_entries(vec![e1, e2]);

        let meta_hits = vec![MetadataHit {
            entry: &db.entries[0],
            score: MatchType::Low,
        }];

        let lyric_hits = vec![LyricHit {
            id: db.entries[1].id,
            rank: 0.1,
            field: LyricMatchField::MainLyric,
            snippet: None,
        }];

        // 隐式搜索 (is_explicit_lyric = false): 求并集，应该包含 e1 和 e2
        let implicit_res = merge_and_sort_hits(&db, meta_hits.clone(), lyric_hits.clone(), false);
        assert_eq!(implicit_res.len(), 2);

        // 显式歌词搜索 (is_explicit_lyric = true): 限定元数据交集，e2 在元数据中不存在，故被排除
        let explicit_res = merge_and_sort_hits(&db, meta_hits, lyric_hits, true);
        assert_eq!(explicit_res.len(), 0);
    }

    // 维度 5：悬挂 ID 与空数据容错测试
    #[test]
    fn test_dangling_id_and_empty_inputs() {
        let e1 = make_song(
            "song1.ttml",
            1_000,
            &["Track 1"],
            &["Artist 1"],
            &[],
            &[],
            &[],
            &[],
        );

        let db = LyricIndexDB::from_entries(vec![e1]);

        // 包含不存在的悬挂 ID (999999)
        let lyric_hits = vec![LyricHit {
            id: LyricId::from_u64(999_999).unwrap(),
            rank: 0.1,
            field: LyricMatchField::MainLyric,
            snippet: None,
        }];

        let res = merge_and_sort_hits(&db, vec![], lyric_hits, false);
        assert!(res.is_empty());

        // 输入全空
        let empty_res = merge_and_sort_hits(&db, vec![], vec![], false);
        assert!(empty_res.is_empty());
    }
}
