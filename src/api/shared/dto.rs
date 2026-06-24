use compact_str::CompactString;
use serde::Serialize;

use crate::core::models::SongEntry;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SongItem {
    pub filename: String,

    pub music_names: Box<[CompactString]>,
    pub artist_names: Box<[CompactString]>,
    pub album_names: Box<[CompactString]>,

    pub ncm_music_ids: Box<[CompactString]>,
    pub qq_music_ids: Box<[CompactString]>,
    pub apple_music_ids: Box<[CompactString]>,
    pub spotify_ids: Box<[CompactString]>,

    pub isrcs: Box<[CompactString]>,

    pub author_ids: Box<[CompactString]>,
    pub author_usernames: Box<[CompactString]>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub lyrics: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Serialize)]
pub struct SearchData {
    pub items: Vec<SongItem>,
}

#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub status: u16,
    pub data: T,
}

pub fn map_song_to_item(
    song: &SongEntry,
    lyrics: Option<String>,
    format: Option<String>,
) -> SongItem {
    SongItem {
        filename: song.filename.to_string(),
        music_names: song.track_names.clone(),
        artist_names: song.artist_names.clone(),
        album_names: song.album_names.clone(),

        ncm_music_ids: song.ncm_music_ids.clone(),
        qq_music_ids: song.qq_music_ids.clone(),
        apple_music_ids: song.apple_music_ids.clone(),
        spotify_ids: song.spotify_ids.clone(),

        isrcs: song.isrcs.clone(),

        author_ids: song.author_ids.clone(),
        author_usernames: song.author_usernames.clone(),

        lyrics,
        format,
    }
}
