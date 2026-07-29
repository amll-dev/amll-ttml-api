use compact_str::CompactString;
use serde::Serialize;

use crate::core::models::SongEntry;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SongItem {
    pub id: u64,
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

    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_context: Option<MatchContext>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MatchContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
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
    match_context: Option<MatchContext>,
) -> SongItem {
    SongItem {
        id: song.id,
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

        match_context,
    }
}

#[cfg(test)]
mod tests {
    use compact_str::CompactString;

    use super::*;
    use crate::utils::id::generate_file_id;

    #[test]
    fn map_song_to_item_includes_id() {
        let filename = "1768754400682-250306205-r6IrpmBd.ttml";
        let song = SongEntry {
            id: generate_file_id(filename),
            filename: CompactString::new(filename),
            timestamp: 1,
            track_names: vec![CompactString::new("ME!")].into_boxed_slice(),
            artist_names: Box::default(),
            album_names: Box::default(),
            normalized_track_names: vec![CompactString::new("me!")].into_boxed_slice(),
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

        let item = map_song_to_item(&song, None, None, None);
        assert_eq!(item.id, song.id);
        assert_eq!(item.filename, filename);

        let json = serde_json::to_value(&item).unwrap();
        assert!(json.get("id").unwrap().is_number());
        assert_eq!(json.get("id").unwrap().as_u64().unwrap(), song.id);
    }
}
