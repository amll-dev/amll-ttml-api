use serde::Serialize;

use crate::{
    core::{
        LyricId,
        models::SongEntry,
    },
    utils::ttml::TTMLFormatResult,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LrclibSongItem {
    pub id: LyricId,
    pub name: String,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: String,
    pub duration: f64,
    pub instrumental: bool,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
}

pub fn map_to_lrclib_item(
    entry: &SongEntry,
    formatted: Option<&TTMLFormatResult>,
) -> LrclibSongItem {
    let track_name = entry
        .track_names
        .first()
        .map(ToString::to_string)
        .unwrap_or_default();

    let (plain_lyrics, synced_lyrics, duration) = formatted.map_or((None, None, 0.0), |f| {
        (f.plain_lyrics.clone(), f.synced_lyrics.clone(), f.duration)
    });

    LrclibSongItem {
        id: entry.id,
        name: track_name.clone(),
        track_name,
        artist_name: entry
            .artist_names
            .first()
            .map(ToString::to_string)
            .unwrap_or_default(),
        album_name: entry
            .album_names
            .first()
            .map(ToString::to_string)
            .unwrap_or_default(),
        duration,
        instrumental: false,
        plain_lyrics,
        synced_lyrics,
    }
}

#[cfg(test)]
mod tests {
    use compact_str::CompactString;

    use super::*;

    fn make_test_entry() -> SongEntry {
        SongEntry {
            id: LyricId::from_u64_masked(LyricId::MAX),
            filename: CompactString::new("test.ttml"),
            timestamp: 123_456_789,
            track_names: vec![
                CompactString::new("Primary Title"),
                CompactString::new("Secondary Title"),
            ]
            .into_boxed_slice(),
            artist_names: vec![CompactString::new("Taylor Swift")].into_boxed_slice(),
            album_names: Box::default(),
            normalized_track_names: vec![
                CompactString::new("primary title"),
                CompactString::new("secondary title"),
            ]
            .into_boxed_slice(),
            normalized_artist_names: vec![CompactString::new("taylor swift")].into_boxed_slice(),
            normalized_album_names: Box::default(),
            ncm_music_ids: Box::default(),
            qq_music_ids: Box::default(),
            apple_music_ids: Box::default(),
            spotify_ids: Box::default(),
            isrcs: Box::default(),
            author_ids: Box::default(),
            author_usernames: Box::default(),
        }
    }

    #[test]
    fn test_map_to_lrclib_item_basic() {
        let entry = make_test_entry();
        let item = map_to_lrclib_item(&entry, None);

        assert_eq!(item.id, LyricId::from_u64_masked(LyricId::MAX));

        assert_eq!(item.track_name, "Primary Title");
        assert_eq!(item.artist_name, "Taylor Swift");

        assert_eq!(item.album_name, "");

        assert_eq!(item.duration, 0.0);
        assert!(!item.instrumental);
        assert_eq!(item.plain_lyrics, None);
        assert_eq!(item.synced_lyrics, None);
    }
}
