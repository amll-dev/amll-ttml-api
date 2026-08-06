use std::borrow::Cow;

use compact_str::CompactString;

use super::{
    compare::{
        compare_artists,
        compare_name,
    },
    query::PreparedQuery,
    types::{
        ArtistMatchType,
        MatchType,
        NameMatchType,
    },
};
use crate::core::models::SongEntry;

const TITLE_WEIGHT: f64 = 1.0;
const ARTIST_WEIGHT: f64 = 1.0;
const ALBUM_WEIGHT: f64 = 0.4;
const MAX_SINGLE_SCORE: f64 = 10.0;
const FULL_SCORE_BASE: f64 = MAX_SINGLE_SCORE * (TITLE_WEIGHT + ARTIST_WEIGHT + ALBUM_WEIGHT);

const SCORE_THRESHOLDS: &[(f64, MatchType)] = &[
    (21.0, MatchType::Perfect),
    (19.0, MatchType::VeryHigh),
    (17.0, MatchType::High),
    (15.0, MatchType::PrettyHigh),
    (11.0, MatchType::Medium),
    (6.5, MatchType::Low),
    (2.5, MatchType::VeryLow),
];

/// 把归一化后的总分映射到匹配档位。
fn classify(normalized_score: f64) -> MatchType {
    for &(threshold, match_type) in SCORE_THRESHOLDS {
        if normalized_score >= threshold {
            return match_type;
        }
    }
    MatchType::NoMatch
}

pub fn score_entry(query: &PreparedQuery, entry: &SongEntry) -> MatchType {
    if query.track_name.is_none()
        && query.artist_name.is_none()
        && query.album_name.is_none()
        && query.global_keyword.is_some()
    {
        let q_norm = query.global_keyword.as_deref().unwrap();
        return score_global_keyword(q_norm, entry);
    }

    let q_title = query
        .track_name
        .as_deref()
        .or(query.global_keyword.as_deref());
    let q_artist = query
        .artist_name
        .as_deref()
        .or(query.global_keyword.as_deref());
    let q_album = query.album_name.as_deref();

    let title_match = entry
        .normalized_track_names
        .iter()
        .map(|name| compare_name(q_title, Some(name.as_str())))
        .max_by_key(|m| *m as u8)
        .unwrap_or(NameMatchType::NoMatch);

    let artist_strs: Vec<&str> = entry
        .normalized_artist_names
        .iter()
        .map(CompactString::as_str)
        .collect();
    let q_artists: Vec<&str> = q_artist.into_iter().collect();
    let artist_match =
        compare_artists(Some(&q_artists), Some(&artist_strs)).unwrap_or(ArtistMatchType::NoMatch);

    let album_match = entry
        .normalized_album_names
        .iter()
        .map(|name| compare_name(q_album, Some(name.as_str())))
        .max_by_key(|m| *m as u8)
        .unwrap_or(NameMatchType::NoMatch);

    let mut total_score = 0.0;
    total_score = f64::from(title_match as u8).mul_add(TITLE_WEIGHT, total_score);
    total_score = f64::from(artist_match as u8).mul_add(ARTIST_WEIGHT, total_score);
    total_score = f64::from(album_match as u8).mul_add(ALBUM_WEIGHT, total_score);

    let mut possible_score = 0.0;
    if q_title.is_some() {
        possible_score = MAX_SINGLE_SCORE.mul_add(TITLE_WEIGHT, possible_score);
    }
    if q_artist.is_some() {
        possible_score = MAX_SINGLE_SCORE.mul_add(ARTIST_WEIGHT, possible_score);
    }
    if q_album.is_some() {
        possible_score = MAX_SINGLE_SCORE.mul_add(ALBUM_WEIGHT, possible_score);
    }

    let normalized_score = if possible_score > 0.0 && possible_score < FULL_SCORE_BASE {
        total_score * (FULL_SCORE_BASE / possible_score)
    } else {
        total_score
    };

    classify(normalized_score)
}

/// 纯全局关键词搜索
fn score_global_keyword(q_norm: &str, entry: &SongEntry) -> MatchType {
    let mut remainder = Cow::Borrowed(q_norm);
    let mut artist_found = false;
    let mut album_found = false;

    // 先删除歌手
    for db_artist in &entry.normalized_artist_names {
        let db_artist_str = db_artist.as_str();
        if !db_artist_str.is_empty() && remainder.contains(db_artist_str) {
            let new_rem = remainder.replace(db_artist_str, "");
            remainder = Cow::Owned(new_rem.trim().to_string());
            artist_found = true;
            break;
        }
    }

    // 再删除专辑
    for db_album in &entry.normalized_album_names {
        let db_album_str = db_album.as_str();
        if !db_album_str.is_empty() && remainder.contains(db_album_str) {
            // 防止同名主打歌导致剩余字符串变成空串
            let test_remainder = remainder.replace(db_album_str, "");
            let trimmed = test_remainder.trim();
            if !trimmed.is_empty() {
                remainder = Cow::Owned(trimmed.to_string());
            }
            album_found = true;
            break;
        }
    }

    // 最后计分
    let title_score = f64::from(
        entry
            .normalized_track_names
            .iter()
            .map(|name| compare_name(Some(&remainder), Some(name.as_str())))
            .max_by_key(|m| *m as u8)
            .unwrap_or(NameMatchType::NoMatch) as u8,
    );

    let artist_score = if artist_found {
        f64::from(ArtistMatchType::Perfect as u8)
    } else {
        f64::from(
            compare_artists(Some(&[q_norm]), Some(&entry.normalized_artist_names))
                .unwrap_or(ArtistMatchType::NoMatch) as u8,
        )
    };

    let album_score = if album_found {
        f64::from(NameMatchType::Perfect as u8)
    } else {
        f64::from(
            entry
                .normalized_album_names
                .iter()
                .map(|name| compare_name(Some(q_norm), Some(name.as_str())))
                .max_by_key(|m| *m as u8)
                .unwrap_or(NameMatchType::NoMatch) as u8,
        )
    };

    let total_score = album_score.mul_add(
        ALBUM_WEIGHT,
        artist_score.mul_add(ARTIST_WEIGHT, title_score * TITLE_WEIGHT),
    );

    // 动态计算全局搜索的提供信息量基准线
    let mut possible_score = 0.0;
    // 如果歌手被消除挖出，或者在兜底比对中拿到了分数，判定歌手维度活跃
    if artist_found || artist_score > 0.0 {
        possible_score = MAX_SINGLE_SCORE.mul_add(ARTIST_WEIGHT, possible_score);
    }
    // 如果专辑被消除挖出，或者在兜底比对中拿到了分数，判定专辑维度活跃
    if album_found || album_score > 0.0 {
        possible_score = MAX_SINGLE_SCORE.mul_add(ALBUM_WEIGHT, possible_score);
    }
    // 如果剔除后不为空，或者歌名比对拿到了分数，判定歌名维度活跃
    if !remainder.is_empty() || title_score > 0.0 {
        possible_score = MAX_SINGLE_SCORE.mul_add(TITLE_WEIGHT, possible_score);
    }

    // 如果什么都没对上，使用全量分母
    if possible_score == 0.0 {
        possible_score = FULL_SCORE_BASE;
    }

    // 等比例数学放大
    let normalized_score = total_score * (FULL_SCORE_BASE / possible_score);

    classify(normalized_score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        matcher::test_support::make_entry,
        models::SearchQuery,
    };

    #[test]
    fn score_perfect_match() {
        let query = SearchQuery {
            track_name: Some("ME!".into()),
            artist_name: Some("Taylor Swift".into()),
            ..Default::default()
        };
        let prepared = PreparedQuery::from_search_query(&query);
        let entry = make_entry(&["ME!"], &["Taylor Swift"], &["Lover"]);
        let score = score_entry(&prepared, &entry);
        assert!(score >= MatchType::VeryHigh);
    }

    #[test]
    fn score_global_keyword_with_artist_extraction() {
        let query = SearchQuery {
            global_keyword: Some("Taylor Swift ME!".into()),
            ..Default::default()
        };
        let prepared = PreparedQuery::from_search_query(&query);
        let entry = make_entry(&["ME!"], &["Taylor Swift"], &["Lover"]);
        let score = score_entry(&prepared, &entry);
        assert!(score >= MatchType::High);
    }

    #[test]
    fn score_no_match() {
        let query = SearchQuery {
            track_name: Some("Completely Different Song".into()),
            ..Default::default()
        };
        let prepared = PreparedQuery::from_search_query(&query);
        let entry = make_entry(&["ME!"], &["Taylor Swift"], &["Lover"]);
        let score = score_entry(&prepared, &entry);
        assert_eq!(score, MatchType::NoMatch);
    }
}
