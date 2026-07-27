use std::collections::HashMap;

use compact_str::CompactString;
use serde::Deserialize;

use crate::utils::{
    id::generate_file_id,
    matcher::normalize_name_for_comparison,
};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RawIndexEntry {
    pub raw_lyric_file: String,
    pub metadata: Vec<(String, Vec<String>)>,
}

#[derive(Clone, Debug)]
pub struct SongEntry {
    pub id: u64,
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

impl From<RawIndexEntry> for SongEntry {
    fn from(raw: RawIndexEntry) -> Self {
        // 格式: 时间戳-作者ID-随机字符串.ttml
        // 示例: 1768754400682-250306205-r6IrpmBd.ttml
        let timestamp = raw
            .raw_lyric_file
            .split('-')
            .next()
            .and_then(|ts_str| ts_str.parse::<u64>().ok())
            .unwrap_or(0);

        let id = generate_file_id(&raw.raw_lyric_file);

        let mut song = Self {
            id,
            filename: CompactString::new(&raw.raw_lyric_file),
            timestamp,

            track_names: Box::default(),
            artist_names: Box::default(),
            album_names: Box::default(),

            normalized_track_names: Box::default(),
            normalized_artist_names: Box::default(),
            normalized_album_names: Box::default(),

            ncm_music_ids: Box::default(),
            qq_music_ids: Box::default(),
            apple_music_ids: Box::default(),
            spotify_ids: Box::default(),

            isrcs: Box::default(),

            author_ids: Box::default(),
            author_usernames: Box::default(),
        };

        for (key, values) in raw.metadata {
            let compact_values: Box<[CompactString]> =
                values.into_iter().map(CompactString::from).collect();

            match key.as_str() {
                "musicName" => song.track_names = compact_values,
                "artists" => song.artist_names = compact_values,
                "album" => song.album_names = compact_values,

                "ncmMusicId" => song.ncm_music_ids = compact_values,
                "qqMusicId" => song.qq_music_ids = compact_values,
                "appleMusicId" => song.apple_music_ids = compact_values,
                "spotifyId" => song.spotify_ids = compact_values,

                "isrc" => song.isrcs = compact_values,

                "ttmlAuthorGithub" => song.author_ids = compact_values,
                "ttmlAuthorGithubLogin" => song.author_usernames = compact_values,
                _ => {}
            }
        }

        let norm_tracks: Vec<CompactString> = song
            .track_names
            .iter()
            .map(|s| CompactString::from(normalize_name_for_comparison(s)))
            .collect();
        let norm_artists: Vec<CompactString> = song
            .artist_names
            .iter()
            .map(|s| CompactString::from(normalize_name_for_comparison(s)))
            .collect();
        let norm_albums: Vec<CompactString> = song
            .album_names
            .iter()
            .map(|s| CompactString::from(normalize_name_for_comparison(s)))
            .collect();

        song.normalized_track_names = norm_tracks.into_boxed_slice();
        song.normalized_artist_names = norm_artists.into_boxed_slice();
        song.normalized_album_names = norm_albums.into_boxed_slice();

        song
    }
}

#[derive(Default)]
pub struct LyricIndexDB {
    pub entries: Vec<SongEntry>,
    pub id_idx: HashMap<u64, usize>,

    pub ncm_idx: HashMap<CompactString, Vec<usize>>,
    pub qq_idx: HashMap<CompactString, Vec<usize>>,
    pub apple_idx: HashMap<CompactString, Vec<usize>>,
    pub spotify_idx: HashMap<CompactString, Vec<usize>>,

    pub isrc_idx: HashMap<CompactString, Vec<usize>>,

    pub author_id_idx: HashMap<CompactString, Vec<usize>>,
    pub author_username_idx: HashMap<CompactString, Vec<usize>>,
}

#[derive(Default)]
pub struct IdQuery {
    pub id: Option<u64>,
    pub filename: Option<String>,

    pub ncm_music_ids: Vec<String>,
    pub qq_music_ids: Vec<String>,
    pub apple_music_ids: Vec<String>,
    pub spotify_ids: Vec<String>,

    pub isrcs: Vec<String>,
}

#[derive(Default)]
pub struct SearchQuery {
    pub global_keyword: Option<String>,

    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,

    pub author_id: Option<String>,
    pub author_username: Option<String>,
}
