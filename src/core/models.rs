use std::collections::HashMap;

use compact_str::CompactString;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RawIndexEntry {
    pub raw_lyric_file: String,
    pub metadata: Vec<(String, Vec<String>)>,
}

#[derive(Clone, Debug)]
pub struct SongEntry {
    pub filename: CompactString,
    pub timestamp: u64,

    pub track_names: Box<[CompactString]>,
    pub artist_names: Box<[CompactString]>,
    pub album_names: Box<[CompactString]>,

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

        let mut song = Self {
            filename: CompactString::new(&raw.raw_lyric_file),
            timestamp,

            track_names: Box::default(),
            artist_names: Box::default(),
            album_names: Box::default(),

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

        song
    }
}

pub struct LyricIndexDB {
    pub entries: Vec<SongEntry>,

    pub ncm_idx: HashMap<CompactString, Vec<usize>>,
    pub qq_idx: HashMap<CompactString, Vec<usize>>,
    pub apple_idx: HashMap<CompactString, Vec<usize>>,
    pub spotify_idx: HashMap<CompactString, Vec<usize>>,

    pub isrc_idx: HashMap<CompactString, Vec<usize>>,
}

pub struct IdQuery {
    pub ncm_music_id: Option<String>,
    pub qq_music_id: Option<String>,
    pub apple_music_id: Option<String>,
    pub spotify_id: Option<String>,

    pub isrc: Option<String>,
}

impl IdQuery {
    pub fn from_http_query(pairs: &[(String, String)]) -> Option<Self> {
        let mut query = Self {
            ncm_music_id: None,
            qq_music_id: None,
            apple_music_id: None,
            spotify_id: None,

            isrc: None,
        };
        let mut has_param = false;

        for (k, v) in pairs {
            let val = Some(v.clone());
            match k.as_str() {
                "ncmMusicId" => {
                    query.ncm_music_id = val;
                    has_param = true;
                }
                "qqMusicId" => {
                    query.qq_music_id = val;
                    has_param = true;
                }
                "appleMusicId" => {
                    query.apple_music_id = val;
                    has_param = true;
                }
                "spotifyId" => {
                    query.spotify_id = val;
                    has_param = true;
                }

                "isrc" => {
                    query.isrc = val;
                    has_param = true;
                }
                _ => {}
            }
        }

        if has_param { Some(query) } else { None }
    }
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

impl SearchQuery {
    pub fn from_http_query(pairs: &[(String, String)]) -> Option<Self> {
        let mut query = Self::default();
        let mut has_param = false;

        for (k, v) in pairs {
            let val = Some(v.clone());
            match k.as_str() {
                "q" => {
                    query.global_keyword = val;
                    has_param = true;
                }

                "musicName" | "trackNames" | "trackName" | "track_name" | "track_names" => {
                    query.track_name = val;
                    has_param = true;
                }
                "artists" | "artistNames" | "artistName" | "artist_name" | "artist_names" => {
                    query.artist_name = val;
                    has_param = true;
                }
                "album" | "albumNames" | "albumName" | "album_name" | "album_names" => {
                    query.album_name = val;
                    has_param = true;
                }

                "ttmlAuthorGithub" | "authorIds" | "authorId" | "author_id" | "author_ids" => {
                    query.author_id = val;
                    has_param = true;
                }
                "ttmlAuthorGithubLogin"
                | "authorUserNames"
                | "authorUserName"
                | "author_username"
                | "author_usernames" => {
                    query.author_username = val;
                    has_param = true;
                }
                _ => {}
            }
        }

        if has_param { Some(query) } else { None }
    }
}
