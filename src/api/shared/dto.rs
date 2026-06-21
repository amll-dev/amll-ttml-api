use compact_str::CompactString;
use serde::Serialize;

use crate::core::models::SongEntry;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponseEntry {
    pub filename: String,

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

    pub synced_lyrics: Option<String>,
    pub plain_lyrics: Option<String>,
}

pub fn map_song_to_dto(song: &SongEntry, synced_lyrics: Option<String>) -> ApiResponseEntry {
    ApiResponseEntry {
        filename: song.filename.to_string(),
        track_names: song.track_names.clone(),
        artist_names: song.artist_names.clone(),
        album_names: song.album_names.clone(),

        ncm_music_ids: song.ncm_music_ids.clone(),
        qq_music_ids: song.qq_music_ids.clone(),
        apple_music_ids: song.apple_music_ids.clone(),
        spotify_ids: song.spotify_ids.clone(),

        isrcs: song.isrcs.clone(),

        author_ids: song.author_ids.clone(),
        author_usernames: song.author_usernames.clone(),

        synced_lyrics,
        plain_lyrics: None,
    }
}
