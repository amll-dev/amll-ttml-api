//! 子模块测试共用的 [`SongEntry`] 构造器。

use compact_str::CompactString;

use super::normalize::normalize_name_for_comparison;
use crate::core::{
    LyricId,
    models::SongEntry,
};

pub fn make_entry(track_names: &[&str], artist_names: &[&str], album_names: &[&str]) -> SongEntry {
    make_full_entry(track_names, artist_names, album_names, &[], &[])
}

pub fn make_full_entry(
    track_names: &[&str],
    artist_names: &[&str],
    album_names: &[&str],
    author_ids: &[&str],
    author_usernames: &[&str],
) -> SongEntry {
    let normalize = |names: &[&str]| -> Box<[CompactString]> {
        names
            .iter()
            .map(|s| CompactString::from(normalize_name_for_comparison(s)))
            .collect()
    };
    let raw = |names: &[&str]| -> Box<[CompactString]> {
        names.iter().map(|s| CompactString::new(*s)).collect()
    };

    SongEntry {
        id: LyricId::from_filename("test.ttml"),
        filename: CompactString::new("test.ttml"),
        timestamp: 0,
        track_names: raw(track_names),
        artist_names: raw(artist_names),
        album_names: raw(album_names),
        normalized_track_names: normalize(track_names),
        normalized_artist_names: normalize(artist_names),
        normalized_album_names: normalize(album_names),
        ncm_music_ids: Box::default(),
        qq_music_ids: Box::default(),
        apple_music_ids: Box::default(),
        spotify_ids: Box::default(),
        isrcs: Box::default(),
        author_ids: raw(author_ids),
        author_usernames: raw(author_usernames),
    }
}
