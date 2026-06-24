use std::{
    collections::HashSet,
    sync::OnceLock,
};

use ferrous_opencc::{
    OpenCC,
    config::BuiltinConfig,
};

use crate::{
    core::models::{
        SearchQuery,
        SongEntry,
    },
    utils::string::contains_ignore_ascii_case,
};

static OPENCC: OnceLock<OpenCC> = OnceLock::new();

fn get_opencc() -> &'static OpenCC {
    OPENCC.get_or_init(|| {
        OpenCC::from_config(BuiltinConfig::Tw2s).expect("Failed to initialize OpenCC dictionary")
    })
}

pub fn convert_tw2s(input: &str) -> String {
    get_opencc().convert(input)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchType {
    NoMatch = 0,
    VeryLow = 1,
    Low = 2,
    Medium = 3,
    PrettyHigh = 4,
    High = 5,
    VeryHigh = 6,
    Perfect = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameMatchType {
    NoMatch = 0,
    Low = 2,
    Medium = 4,
    High = 6,
    VeryHigh = 8,
    Perfect = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtistMatchType {
    NoMatch = 0,
    Low = 2,
    Medium = 4,
    High = 6,
    VeryHigh = 8,
    Perfect = 10,
}

/// 计算两个字符串的归一化 Levenshtein 相似度，并转换为百分比。
fn compute_text_same(text1: &str, text2: &str) -> f64 {
    strsim::normalized_levenshtein(text1, text2) * 100.0
}

/// 归一化名称字符串
fn normalize_name_for_comparison(name: &str) -> String {
    let replaced = name
        .replace('’', "'")
        .replace('，', ",")
        .replace(['（', '【', '['], " (")
        .replace(['）', '】', ']'], ") ")
        .replace("acoustic version", "acoustic");

    replaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

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
        .track_names
        .iter()
        .map(|name| compare_name(q_title, Some(&name.to_lowercase())))
        .max_by_key(|m| *m as u8)
        .unwrap_or(NameMatchType::NoMatch);

    let artist_strs: Vec<String> = entry
        .artist_names
        .iter()
        .map(|name| name.as_str().to_lowercase())
        .collect();
    let q_artists: Vec<&str> = q_artist.into_iter().collect();
    let artist_match =
        compare_artists(Some(&q_artists), Some(&artist_strs)).unwrap_or(ArtistMatchType::NoMatch);

    let album_match = entry
        .album_names
        .iter()
        .map(|name| compare_name(q_album, Some(&name.to_lowercase())))
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

    for &(threshold, match_type) in SCORE_THRESHOLDS {
        if normalized_score >= threshold {
            return match_type;
        }
    }

    MatchType::NoMatch
}

/// 纯全局关键词搜索
fn score_global_keyword(q_norm: &str, entry: &SongEntry) -> MatchType {
    let mut remainder = q_norm.to_string();
    let mut artist_found = false;
    let mut album_found = false;

    // 先删除歌手
    for artist in &entry.artist_names {
        let db_artist = artist.as_str().to_lowercase();
        if !db_artist.is_empty() && remainder.contains(&db_artist) {
            remainder = remainder.replace(&db_artist, "").trim().to_string();
            artist_found = true;
            break;
        }
    }

    // 再删除专辑
    for album in &entry.album_names {
        let db_album = album.as_str().to_lowercase();
        if !db_album.is_empty() && remainder.contains(&db_album) {
            // 防止同名主打歌导致剩余字符串变成空串
            let test_remainder = remainder.replace(&db_album, "").trim().to_string();
            if !test_remainder.is_empty() {
                remainder = test_remainder;
            }
            album_found = true;
            break;
        }
    }

    // 最后计分
    let title_score = f64::from(
        entry
            .track_names
            .iter()
            .map(|name| compare_name(Some(&remainder), Some(&name.to_lowercase())))
            .max_by_key(|m| *m as u8)
            .unwrap_or(NameMatchType::NoMatch) as u8,
    );

    let artist_score = if artist_found {
        f64::from(ArtistMatchType::Perfect as u8)
    } else {
        let artist_strs: Vec<String> = entry
            .artist_names
            .iter()
            .map(|name| name.as_str().to_lowercase())
            .collect();
        f64::from(
            compare_artists(Some(&[q_norm]), Some(&artist_strs)).unwrap_or(ArtistMatchType::NoMatch)
                as u8,
        )
    };

    let album_score = if album_found {
        f64::from(NameMatchType::Perfect as u8)
    } else {
        f64::from(
            entry
                .album_names
                .iter()
                .map(|name| compare_name(Some(q_norm), Some(&name.to_lowercase())))
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

    for &(threshold, match_type) in SCORE_THRESHOLDS {
        if normalized_score >= threshold {
            return match_type;
        }
    }
    MatchType::NoMatch
}

fn check_dash_paren_equivalence(s_dash: &str, s_paren: &str) -> bool {
    let is_dash = s_dash.contains(" - ") && !s_dash.contains('(');
    let is_paren = s_paren.contains('(') && !s_paren.contains(" - ");

    if is_dash
        && is_paren
        && let Some((base, suffix)) = s_dash.split_once(" - ")
    {
        return format!("{} ({})", base.trim(), suffix.trim()) == s_paren;
    }
    false
}

fn compare_name(name1_opt: Option<&str>, name2_opt: Option<&str>) -> NameMatchType {
    let (Some(name1_norm), Some(name2_norm)) = (name1_opt, name2_opt) else {
        return NameMatchType::NoMatch;
    };

    if name1_norm.trim() == name2_norm.trim() {
        return NameMatchType::Perfect;
    }

    let name1 = normalize_name_for_comparison(name1_norm);
    let name2 = normalize_name_for_comparison(name2_norm);
    if name1.trim() == name2.trim() {
        return NameMatchType::Perfect;
    }

    if check_dash_paren_equivalence(&name1, &name2) || check_dash_paren_equivalence(&name2, &name1)
    {
        return NameMatchType::VeryHigh;
    }

    let special_suffixes = [
        "deluxe",
        "explicit",
        "special edition",
        "bonus track",
        "feat",
        "with",
    ];
    for suffix in special_suffixes {
        let suffixed_form = format!("({suffix}");
        if (name1.contains(&suffixed_form)
            && !name2.contains(&suffixed_form)
            && name2 == name1.split(&suffixed_form).next().unwrap_or("").trim())
            || (name2.contains(&suffixed_form)
                && !name1.contains(&suffixed_form)
                && name1 == name2.split(&suffixed_form).next().unwrap_or("").trim())
        {
            return NameMatchType::VeryHigh;
        }
    }

    if name1.contains('(')
        && name2.contains('(')
        && let (Some(n1_base), Some(n2_base)) = (name1.split('(').next(), name2.split('(').next())
        && n1_base.trim() == n2_base.trim()
    {
        return NameMatchType::High;
    }

    if (name1.contains('(')
        && !name2.contains('(')
        && name2 == name1.split('(').next().unwrap_or("").trim())
        || (name2.contains('(')
            && !name1.contains('(')
            && name1 == name2.split('(').next().unwrap_or("").trim())
    {
        return NameMatchType::High;
    }

    if name1.chars().count() == name2.chars().count() {
        let count = name1
            .chars()
            .zip(name2.chars())
            .filter(|(c1, c2)| c1 == c2)
            .count();
        let len = name1.chars().count();
        let count_f64 = count as f64;
        let len_f64 = len as f64;
        let ratio = count_f64 / len_f64;
        if (ratio >= 0.8 && len >= 4) || (ratio >= 0.5 && (2..=3).contains(&len)) {
            return NameMatchType::High;
        }
    }

    if compute_text_same(&name1, &name2) > 90.0 {
        return NameMatchType::VeryHigh;
    }
    if compute_text_same(&name1, &name2) > 80.0 {
        return NameMatchType::High;
    }
    if compute_text_same(&name1, &name2) > 68.0 {
        return NameMatchType::Medium;
    }
    if compute_text_same(&name1, &name2) > 55.0 {
        return NameMatchType::Low;
    }

    NameMatchType::NoMatch
}

fn compare_artists<S1: AsRef<str>, S2: AsRef<str>>(
    artists1: Option<&[S1]>,
    artists2: Option<&[S2]>,
) -> Option<ArtistMatchType> {
    const JACCARD_THRESHOLDS: &[(f64, ArtistMatchType)] = &[
        (0.99, ArtistMatchType::Perfect),
        (0.80, ArtistMatchType::VeryHigh),
        (0.60, ArtistMatchType::High),
        (0.40, ArtistMatchType::Medium),
        (0.15, ArtistMatchType::Low),
    ];
    const LEVENSHTEIN_THRESHOLD: f64 = 88.0;

    let list1_raw = artists1?;
    let list2_raw = artists2?;
    if list1_raw.is_empty() || list2_raw.is_empty() {
        return None;
    }

    let list1: Vec<&str> = list1_raw
        .iter()
        .map(AsRef::as_ref)
        .filter(|s| !s.is_empty())
        .collect();
    let list2: Vec<&str> = list2_raw
        .iter()
        .map(AsRef::as_ref)
        .filter(|s| !s.is_empty())
        .collect();

    let is_l1_various = list1
        .iter()
        .any(|s| s.contains("various") || s.contains("群星"));
    let is_l2_various = list2
        .iter()
        .any(|s| s.contains("various") || s.contains("群星"));
    if (is_l1_various && (is_l2_various || list2.len() > 4)) || (is_l2_various && list1.len() > 4) {
        return Some(ArtistMatchType::High);
    }

    let mut intersection_size = 0;
    let mut matched_indices_in_list2 = HashSet::new();

    for artist1 in &list1 {
        let mut best_match_idx = None;
        for (i, artist2) in list2.iter().enumerate() {
            if matched_indices_in_list2.contains(&i) {
                continue;
            }

            if artist2.contains(artist1)
                || artist1.contains(artist2)
                || compute_text_same(artist1, artist2) > LEVENSHTEIN_THRESHOLD
            {
                best_match_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = best_match_idx {
            intersection_size += 1;
            matched_indices_in_list2.insert(idx);
        }
    }

    let union_size = list1.len() + list2.len() - intersection_size;
    if union_size == 0 {
        return Some(ArtistMatchType::Perfect);
    }

    let jaccard_score = intersection_size as f64 / union_size as f64;

    for &(threshold, match_type) in JACCARD_THRESHOLDS {
        if jaccard_score >= threshold {
            return Some(match_type);
        }
    }

    Some(ArtistMatchType::NoMatch)
}

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
            .map(|s| convert_tw2s(s).to_lowercase());
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
                .map(|s| convert_tw2s(s).to_lowercase()),
            artist_name: query
                .artist_name
                .as_deref()
                .map(|s| convert_tw2s(s).to_lowercase()),
            album_name: query
                .album_name
                .as_deref()
                .map(|s| convert_tw2s(s).to_lowercase()),
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

pub fn rough_match(prepared: &PreparedQuery, entry: &SongEntry) -> bool {
    if let Some(ref q) = prepared.global_keyword {
        // 如果用户的超长 Query 里已经完整包含了数据库的歌名或歌手，
        // 则直接把这首歌送进精确排序里
        let is_track_wrapped = entry
            .track_names
            .iter()
            .any(|n| contains_ignore_ascii_case(q, n.as_str()));
        let is_artist_wrapped = entry
            .artist_names
            .iter()
            .any(|n| contains_ignore_ascii_case(q, n.as_str()));

        if !is_track_wrapped || !is_artist_wrapped {
            let mut all_tokens_matched = true;

            for token in &prepared.global_tokens {
                let mut token_matched = false;

                // 双向包含逻辑：
                // A: 数据库字段 包含 Token (常规情况：DB="Taylor Swift", Token="Taylor")
                // B: Token 包含 数据库字段 (极端情况：Token="夜曲周杰伦", DB="夜曲"。)
                // 同时限制 db_val.len() > 1 防止单字母泛滥
                let check_match = |db_val: &str| {
                    contains_ignore_ascii_case(db_val, token)
                        || (db_val.len() > 1 && contains_ignore_ascii_case(token, db_val))
                };

                // 只要当前 token 命中了任意一个字段，该 token 就算验证通过
                if entry.track_names.iter().any(|n| check_match(n.as_str()))
                    || entry.artist_names.iter().any(|n| check_match(n.as_str()))
                    || entry.album_names.iter().any(|n| check_match(n.as_str()))
                {
                    token_matched = true;
                }

                // 如果有任何一个 token 完全没有匹配上，说明这首歌完全不符合用户的混合查询
                if !token_matched {
                    all_tokens_matched = false;
                    break;
                }
            }

            if !all_tokens_matched {
                return false;
            }
        }
    }

    // 具体字段的过滤为包含和 AND 关系
    if let Some(ref val) = prepared.track_name
        && !entry
            .track_names
            .iter()
            .any(|n| contains_ignore_ascii_case(n.as_str(), val))
    {
        return false;
    }

    if let Some(ref val) = prepared.artist_name
        && !entry
            .artist_names
            .iter()
            .any(|n| contains_ignore_ascii_case(n.as_str(), val))
    {
        return false;
    }

    if let Some(ref val) = prepared.album_name
        && !entry
            .album_names
            .iter()
            .any(|n| contains_ignore_ascii_case(n.as_str(), val))
    {
        return false;
    }

    // ID 和用户名严格相等才算匹配
    if let Some(ref val) = prepared.author_id
        && !entry.author_ids.iter().any(|n| n.as_str() == val)
    {
        return false;
    }
    if let Some(ref val) = prepared.author_username
        && !entry.author_usernames.iter().any(|n| n.as_str() == val)
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use compact_str::CompactString;

    use super::*;
    use crate::core::models::SearchQuery;

    fn make_entry(track_names: &[&str], artist_names: &[&str], album_names: &[&str]) -> SongEntry {
        SongEntry {
            filename: CompactString::new("test.ttml"),
            timestamp: 0,
            track_names: track_names.iter().map(|s| CompactString::new(*s)).collect(),
            artist_names: artist_names
                .iter()
                .map(|s| CompactString::new(*s))
                .collect(),
            album_names: album_names.iter().map(|s| CompactString::new(*s)).collect(),
            ncm_music_ids: Box::default(),
            qq_music_ids: Box::default(),
            apple_music_ids: Box::default(),
            spotify_ids: Box::default(),
            isrcs: Box::default(),
            author_ids: Box::default(),
            author_usernames: Box::default(),
        }
    }

    fn make_full_entry(
        track_names: &[&str],
        artist_names: &[&str],
        album_names: &[&str],
        author_ids: &[&str],
        author_usernames: &[&str],
    ) -> SongEntry {
        SongEntry {
            filename: CompactString::new("test.ttml"),
            timestamp: 0,
            track_names: track_names.iter().map(|s| CompactString::new(*s)).collect(),
            artist_names: artist_names
                .iter()
                .map(|s| CompactString::new(*s))
                .collect(),
            album_names: album_names.iter().map(|s| CompactString::new(*s)).collect(),
            ncm_music_ids: Box::default(),
            qq_music_ids: Box::default(),
            apple_music_ids: Box::default(),
            spotify_ids: Box::default(),
            isrcs: Box::default(),
            author_ids: author_ids.iter().map(|s| CompactString::new(*s)).collect(),
            author_usernames: author_usernames
                .iter()
                .map(|s| CompactString::new(*s))
                .collect(),
        }
    }

    fn prepared_from_query(query: &SearchQuery) -> PreparedQuery {
        PreparedQuery::from_search_query(query)
    }

    // --- rough_match tests ---

    #[test]
    fn rough_match_global_keyword_matches_track() {
        let query = SearchQuery {
            global_keyword: Some("ME!".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_entry(&["ME! (feat. Brendon Urie)"], &["Taylor Swift"], &["Lover"]);
        assert!(rough_match(&prepared, &entry));
    }

    #[test]
    fn rough_match_global_keyword_matches_artist() {
        let query = SearchQuery {
            global_keyword: Some("Taylor Swift".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_entry(&["Love Story"], &["Taylor Swift"], &["Fearless"]);
        assert!(rough_match(&prepared, &entry));
    }

    #[test]
    fn rough_match_global_keyword_no_match() {
        let query = SearchQuery {
            global_keyword: Some("NonExistent Artist".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_entry(&["ME!"], &["Taylor Swift"], &["Lover"]);
        assert!(!rough_match(&prepared, &entry));
    }

    #[test]
    fn rough_match_track_name_contains() {
        let query = SearchQuery {
            track_name: Some("Love".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_entry(&["Love Story"], &["Taylor Swift"], &["Fearless"]);
        assert!(rough_match(&prepared, &entry));
    }

    #[test]
    fn rough_match_track_name_no_match() {
        let query = SearchQuery {
            track_name: Some("NonExistent".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_entry(&["ME!"], &["Taylor Swift"], &["Lover"]);
        assert!(!rough_match(&prepared, &entry));
    }

    #[test]
    fn rough_match_artist_name_and() {
        let query = SearchQuery {
            track_name: Some("ME!".into()),
            artist_name: Some("Taylor".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_entry(&["ME!"], &["Taylor Swift"], &["Lover"]);
        assert!(rough_match(&prepared, &entry));
    }

    #[test]
    fn rough_match_artist_name_and_fails() {
        let query = SearchQuery {
            track_name: Some("ME!".into()),
            artist_name: Some("Ed Sheeran".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_entry(&["ME!"], &["Taylor Swift"], &["Lover"]);
        assert!(!rough_match(&prepared, &entry));
    }

    #[test]
    fn rough_match_author_id_exact() {
        let query = SearchQuery {
            author_id: Some("108002475".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_full_entry(
            &["ME!"],
            &["Taylor Swift"],
            &["Lover"],
            &["108002475"],
            &["apoint123"],
        );
        assert!(rough_match(&prepared, &entry));
    }

    #[test]
    fn rough_match_author_id_exact_no_match() {
        let query = SearchQuery {
            author_id: Some("999999999".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_full_entry(
            &["ME!"],
            &["Taylor Swift"],
            &["Lover"],
            &["108002475"],
            &["apoint123"],
        );
        assert!(!rough_match(&prepared, &entry));
    }

    #[test]
    fn rough_match_author_username_exact() {
        let query = SearchQuery {
            author_username: Some("apoint123".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
        let entry = make_full_entry(
            &["ME!"],
            &["Taylor Swift"],
            &["Lover"],
            &["108002475"],
            &["apoint123"],
        );
        assert!(rough_match(&prepared, &entry));
    }

    // --- score_entry tests ---

    #[test]
    fn score_perfect_match() {
        let query = SearchQuery {
            track_name: Some("ME!".into()),
            artist_name: Some("Taylor Swift".into()),
            ..Default::default()
        };
        let prepared = prepared_from_query(&query);
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
        let prepared = prepared_from_query(&query);
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
        let prepared = prepared_from_query(&query);
        let entry = make_entry(&["ME!"], &["Taylor Swift"], &["Lover"]);
        let score = score_entry(&prepared, &entry);
        assert_eq!(score, MatchType::NoMatch);
    }

    // --- convert_tw2s tests ---

    #[test]
    fn convert_traditional_to_simplified() {
        // 繁体 "愛" -> 简体 "爱"
        let result = convert_tw2s("愛");
        assert_eq!(result, "爱");
    }

    #[test]
    fn convert_simplified_unchanged() {
        let result = convert_tw2s("爱");
        assert_eq!(result, "爱");
    }

    // --- normalize_name_for_comparison tests ---

    #[test]
    fn normalize_curly_quotes() {
        let result = normalize_name_for_comparison("it\u{2019}s");
        assert_eq!(result, "it's");
    }

    #[test]
    fn normalize_chinese_comma() {
        let result = normalize_name_for_comparison("A，B");
        assert_eq!(result, "A,B");
    }

    #[test]
    fn normalize_brackets() {
        let result = normalize_name_for_comparison("Song [Deluxe]");
        assert_eq!(result, "Song (Deluxe)");
    }

    #[test]
    fn normalize_multiple_spaces() {
        assert_eq!(normalize_name_for_comparison("A   B"), "A B");
        assert_eq!(normalize_name_for_comparison("A    B"), "A B");
        assert_eq!(normalize_name_for_comparison("  A  B  "), "A B");
    }

    // --- compare_name tests ---

    #[test]
    fn compare_name_perfect_match() {
        assert_eq!(
            compare_name(Some("me!"), Some("me!")),
            NameMatchType::Perfect
        );
    }

    #[test]
    fn compare_name_dash_paren_equivalence() {
        // "Song - Remix" vs "Song (Remix)"
        assert_eq!(
            compare_name(Some("song - remix"), Some("song (remix)")),
            NameMatchType::VeryHigh
        );
    }

    #[test]
    fn compare_name_no_match() {
        assert_eq!(
            compare_name(Some("completely different"), Some("another song")),
            NameMatchType::NoMatch
        );
    }
}
