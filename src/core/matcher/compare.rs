use super::types::{
    ArtistMatchType,
    NameMatchType,
};

/// 计算两个字符串的归一化 Levenshtein 相似度，并转换为百分比。
fn compute_text_same(text1: &str, text2: &str) -> f64 {
    let len1 = text1.chars().count();
    let len2 = text2.chars().count();
    if len1 == 0 || len2 == 0 {
        return 0.0;
    }
    let max_len = len1.max(len2) as f64;
    let min_len = len1.min(len2) as f64;

    // 若理论最大可达到相似度上限低于最低门槛 (55.0)，直接剪枝跳过矩阵分配
    if (min_len / max_len) * 100.0 < 55.0 {
        return 0.0;
    }

    strsim::normalized_levenshtein(text1, text2) * 100.0
}

fn check_dash_paren_equivalence(s_dash: &str, s_paren: &str) -> bool {
    let is_dash = s_dash.contains(" - ") && !s_dash.contains('(');
    let is_paren = s_paren.contains('(') && !s_paren.contains(" - ");

    if is_dash
        && is_paren
        && let Some((base, suffix)) = s_dash.split_once(" - ")
    {
        let base = base.trim();
        let suffix = suffix.trim();
        if let Some(rest) = s_paren.strip_prefix(base) {
            let rest = rest.trim_start();
            if let Some(inner) = rest.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
                return inner.trim() == suffix;
            }
        }
    }
    false
}

pub fn compare_name(name1_opt: Option<&str>, name2_opt: Option<&str>) -> NameMatchType {
    let (Some(name1), Some(name2)) = (name1_opt, name2_opt) else {
        return NameMatchType::NoMatch;
    };

    if name1.trim() == name2.trim() {
        return NameMatchType::Perfect;
    }

    if check_dash_paren_equivalence(name1, name2) || check_dash_paren_equivalence(name2, name1) {
        return NameMatchType::VeryHigh;
    }

    #[expect(clippy::items_after_statements)]
    const SPECIAL_SUFFIX_FORMS: &[&str] = &[
        "(deluxe",
        "(explicit",
        "(special edition",
        "(bonus track",
        "(feat",
        "(with",
    ];
    for &suffixed_form in SPECIAL_SUFFIX_FORMS {
        if (name1.contains(suffixed_form)
            && !name2.contains(suffixed_form)
            && name2 == name1.split(suffixed_form).next().unwrap_or("").trim())
            || (name2.contains(suffixed_form)
                && !name1.contains(suffixed_form)
                && name1 == name2.split(suffixed_form).next().unwrap_or("").trim())
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

    if compute_text_same(name1, name2) > 90.0 {
        return NameMatchType::VeryHigh;
    }
    if compute_text_same(name1, name2) > 80.0 {
        return NameMatchType::High;
    }
    if compute_text_same(name1, name2) > 68.0 {
        return NameMatchType::Medium;
    }
    if compute_text_same(name1, name2) > 55.0 {
        return NameMatchType::Low;
    }

    NameMatchType::NoMatch
}

pub fn compare_artists<S1: AsRef<str>, S2: AsRef<str>>(
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

    let l1_len = list1_raw.iter().filter(|s| !s.as_ref().is_empty()).count();
    let l2_len = list2_raw.iter().filter(|s| !s.as_ref().is_empty()).count();

    if l1_len == 0 || l2_len == 0 {
        return None;
    }

    let is_l1_various = list1_raw
        .iter()
        .map(AsRef::as_ref)
        .any(|s| !s.is_empty() && (s.contains("various") || s.contains("群星")));
    let is_l2_various = list2_raw
        .iter()
        .map(AsRef::as_ref)
        .any(|s| !s.is_empty() && (s.contains("various") || s.contains("群星")));

    if (is_l1_various && (is_l2_various || l2_len > 4)) || (is_l2_various && l1_len > 4) {
        return Some(ArtistMatchType::High);
    }

    let mut intersection_size = 0;
    let mut matched_mask: u64 = 0;

    for artist1 in list1_raw
        .iter()
        .map(AsRef::as_ref)
        .filter(|s| !s.is_empty())
    {
        let mut best_match_idx = None;
        let mut l2_idx = 0;

        for artist2 in list2_raw
            .iter()
            .map(AsRef::as_ref)
            .filter(|s| !s.is_empty())
        {
            if l2_idx < 64 && (matched_mask & (1 << l2_idx)) != 0 {
                l2_idx += 1;
                continue;
            }

            if artist2.contains(artist1)
                || artist1.contains(artist2)
                || compute_text_same(artist1, artist2) > LEVENSHTEIN_THRESHOLD
            {
                best_match_idx = Some(l2_idx);
                break;
            }
            l2_idx += 1;
        }

        if let Some(idx) = best_match_idx {
            intersection_size += 1;
            if idx < 64 {
                matched_mask |= 1 << idx;
            }
        }
    }

    let union_size = l1_len + l2_len - intersection_size;
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

#[cfg(test)]
mod tests {
    use super::*;

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
