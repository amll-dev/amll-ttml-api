use super::normalize::{
    convert_tw2s,
    normalize_name_for_comparison,
};
use crate::core::models::SearchQuery;

pub struct PreparedQuery {
    pub global_keyword: Option<String>,
    pub global_tokens: Vec<String>,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub author_id: Option<String>,
    pub author_username: Option<String>,
}

impl PreparedQuery {
    pub fn from_search_query(query: &SearchQuery) -> Self {
        let global_keyword = query
            .global_keyword
            .as_deref()
            .map(|s| normalize_name_for_comparison(&convert_tw2s(s)));
        let global_tokens = global_keyword
            .as_ref()
            .map(|q| q.split_whitespace().map(ToString::to_string).collect())
            .unwrap_or_default();

        Self {
            global_keyword,
            global_tokens,
            track_name: query
                .track_name
                .as_deref()
                .map(|s| normalize_name_for_comparison(&convert_tw2s(s))),
            artist_name: query
                .artist_name
                .as_deref()
                .map(|s| normalize_name_for_comparison(&convert_tw2s(s))),
            album_name: query
                .album_name
                .as_deref()
                .map(|s| normalize_name_for_comparison(&convert_tw2s(s))),
            author_id: query.author_id.clone(),
            author_username: query.author_username.clone(),
        }
    }

    pub const fn has_text_fields(&self) -> bool {
        self.track_name.is_some()
            || self.artist_name.is_some()
            || self.album_name.is_some()
            || self.global_keyword.is_some()
    }
}
