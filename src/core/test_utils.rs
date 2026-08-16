use compact_str::CompactString;

use crate::core::{
    LyricId,
    matcher::normalize_name_for_comparison,
    models::SongEntry,
};

#[allow(clippy::too_many_arguments)]
pub fn make_song(
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
        id: LyricId::from_filename(filename),
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
