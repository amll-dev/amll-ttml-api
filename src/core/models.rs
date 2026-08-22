use std::collections::HashMap;

use compact_str::CompactString;
use serde::Deserialize;

use crate::core::LyricId;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RawIndexEntry {
    pub raw_lyric_file: String,
    pub metadata: Vec<(String, Vec<String>)>,
}

#[derive(Clone, Debug)]
pub struct SongEntry {
    pub id: LyricId,
    pub filename: CompactString,
    pub timestamp: u64,

    pub track_names: Box<[CompactString]>,
    pub artist_names: Box<[CompactString]>,
    pub album_names: Box<[CompactString]>,

    pub normalized_track_names: Box<[CompactString]>,
    pub normalized_artist_names: Box<[CompactString]>,
    pub normalized_album_names: Box<[CompactString]>,

    pub ncm_music_ids: Box<[CompactString]>,
    pub qq_music_ids: Box<[CompactString]>,
    pub apple_music_ids: Box<[CompactString]>,
    pub spotify_ids: Box<[CompactString]>,

    pub isrcs: Box<[CompactString]>,

    pub author_ids: Box<[CompactString]>,
    pub author_usernames: Box<[CompactString]>,
}

#[derive(Default)]
pub struct LyricIndexDB {
    pub entries: Vec<SongEntry>,
    pub id_idx: HashMap<LyricId, usize>,

    pub ncm_idx: HashMap<CompactString, Vec<usize>>,
    pub qq_idx: HashMap<CompactString, Vec<usize>>,
    pub apple_idx: HashMap<CompactString, Vec<usize>>,
    pub spotify_idx: HashMap<CompactString, Vec<usize>>,

    pub isrc_idx: HashMap<CompactString, Vec<usize>>,

    pub author_id_idx: HashMap<CompactString, Vec<usize>>,
    pub author_username_idx: HashMap<CompactString, Vec<usize>>,
}

impl LyricIndexDB {
    /// 由条目列表组装全部倒排索引
    #[must_use]
    pub fn from_entries(entries: Vec<SongEntry>) -> Self {
        let mut index = Self {
            entries: Vec::with_capacity(entries.len()),
            ..Self::default()
        };

        for song in entries {
            let i = index.entries.len();
            index.id_idx.insert(song.id, i);

            for id in &song.ncm_music_ids {
                index.ncm_idx.entry(id.clone()).or_default().push(i);
            }
            for id in &song.qq_music_ids {
                index.qq_idx.entry(id.clone()).or_default().push(i);
            }
            for id in &song.apple_music_ids {
                index.apple_idx.entry(id.clone()).or_default().push(i);
            }
            for id in &song.spotify_ids {
                index.spotify_idx.entry(id.clone()).or_default().push(i);
            }
            for id in &song.isrcs {
                index.isrc_idx.entry(id.clone()).or_default().push(i);
            }
            for id in &song.author_ids {
                index.author_id_idx.entry(id.clone()).or_default().push(i);
            }
            for id in &song.author_usernames {
                index
                    .author_username_idx
                    .entry(id.clone())
                    .or_default()
                    .push(i);
            }

            index.entries.push(song);
        }

        index
    }
}

#[derive(Default)]
pub struct IdQuery {
    pub id: Option<LyricId>,
    pub filename: Option<String>,

    pub ncm_music_ids: Vec<String>,
    pub qq_music_ids: Vec<String>,
    pub apple_music_ids: Vec<String>,
    pub spotify_ids: Vec<String>,

    pub isrcs: Vec<String>,
}

impl IdQuery {
    /// 是否为基于强唯一标识（ID 或文件名）的精确查询
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        self.id.is_some() || self.filename.is_some()
    }
}

#[derive(Default)]
pub struct SearchQuery {
    pub global_keyword: Option<String>,

    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub lyric_text: Option<String>,

    pub author_id: Option<String>,
    pub author_username: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricMatchField {
    MainLyric,
    BackgroundVocal,
}

#[derive(Debug, Clone)]
pub struct LyricHit {
    pub id: LyricId,
    pub rank: f64,
    pub field: LyricMatchField,
    pub snippet: Option<String>,
}

/// `lyrics/search` 的一条命中结果，包含条目本体和可选的歌词正文命中信息
#[derive(Debug, Clone)]
pub struct LyricSearchResult {
    pub entry: SongEntry,
    pub lyric_hit: Option<LyricHit>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_utils::make_song;

    #[test]
    fn from_entries_empty() {
        let index = LyricIndexDB::from_entries(Vec::new());
        assert!(index.entries.is_empty());
        assert!(index.id_idx.is_empty());
        assert!(index.spotify_idx.is_empty());
    }

    #[test]
    fn from_entries_builds_platform_indices() {
        let index = LyricIndexDB::from_entries(vec![
            make_song(
                "a.ttml",
                1,
                &["Song A"],
                &["Artist X"],
                &["111"],
                &["sp1"],
                &[],
                &[],
            ),
            make_song(
                "b.ttml",
                2,
                &["Song B"],
                &["Artist Y"],
                &[],
                &["sp1"],
                &[],
                &[],
            ),
        ]);

        let mut spotify_hits = index.spotify_idx.get("sp1").cloned().unwrap_or_default();
        spotify_hits.sort_unstable();
        assert_eq!(spotify_hits, vec![0, 1]);

        assert_eq!(index.ncm_idx.get("111").cloned(), Some(vec![0]));
        assert_eq!(index.entries.len(), 2);
    }

    #[test]
    fn from_entries_id_collision_last_wins() {
        let a = make_song("a.ttml", 1, &["Song A"], &["Artist X"], &[], &[], &[], &[]);
        let mut b = make_song("b.ttml", 2, &["Song B"], &["Artist Y"], &[], &[], &[], &[]);
        b.id = a.id;
        let colliding_id = b.id;

        let index = LyricIndexDB::from_entries(vec![a, b]);
        assert_eq!(index.id_idx.get(&colliding_id), Some(&1));
    }
}
