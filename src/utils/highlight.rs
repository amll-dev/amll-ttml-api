pub fn extract_lyric_context(lyric_text: &str, keyword: &str) -> Option<String> {
    if lyric_text.trim().is_empty() || keyword.trim().is_empty() {
        return None;
    }

    let lines: Vec<&str> = lyric_text.lines().collect();
    if lines.is_empty() {
        return None;
    }

    let kw_lower = keyword.to_lowercase();

    let matched_idx = lines
        .iter()
        .position(|line| line.to_lowercase().contains(&kw_lower))?;

    let start_idx = matched_idx.saturating_sub(1);
    let end_idx = (matched_idx + 2).min(lines.len());

    let context_lines = &lines[start_idx..end_idx];
    let joined_context = context_lines.join("\n");

    Some(highlight_keyword(&joined_context, keyword))
}

pub fn highlight_keyword(text: &str, keyword: &str) -> String {
    if keyword.is_empty() || text.is_empty() {
        return text.to_string();
    }

    let kw_lower = keyword.to_lowercase();
    let kw_char_count = kw_lower.chars().count();
    let mut result = String::with_capacity(text.len() + 32);
    let mut last_end_byte = 0;

    let char_boundaries: Vec<(usize, char)> = text.char_indices().collect();
    let total_chars = char_boundaries.len();
    let mut i = 0;

    while i < total_chars {
        if i + kw_char_count <= total_chars {
            let start_byte = char_boundaries[i].0;
            let end_byte = if i + kw_char_count < total_chars {
                char_boundaries[i + kw_char_count].0
            } else {
                text.len()
            };

            let candidate = &text[start_byte..end_byte];

            if candidate.to_lowercase() == kw_lower {
                result.push_str(&text[last_end_byte..start_byte]);
                result.push_str("<mark>");
                result.push_str(candidate);
                result.push_str("</mark>");

                last_end_byte = end_byte;
                i += kw_char_count;
                continue;
            }
        }
        i += 1;
    }

    result.push_str(&text[last_end_byte..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_keyword_basic_casing_preserved() {
        let text = "In the END, it doesn't matter";
        let kw = "end";
        let result = highlight_keyword(text, kw);
        assert_eq!(result, "In the <mark>END</mark>, it doesn't matter");
    }

    #[test]
    fn test_highlight_keyword_multiple_matches() {
        let text = "hello world, HELLO universe";
        let kw = "hello";
        let result = highlight_keyword(text, kw);
        assert_eq!(
            result,
            "<mark>hello</mark> world, <mark>HELLO</mark> universe"
        );
    }

    #[test]
    fn test_highlight_keyword_chinese() {
        let text = "为你弹奏肖邦的夜曲，纪念我死去的爱情";
        let kw = "肖邦";
        let result = highlight_keyword(text, kw);
        assert_eq!(result, "为你弹奏<mark>肖邦</mark>的夜曲，纪念我死去的爱情");
    }

    #[test]
    fn test_highlight_keyword_no_match() {
        let text = "Hello World";
        let kw = "abc";
        let result = highlight_keyword(text, kw);
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_highlight_keyword_empty_inputs() {
        assert_eq!(highlight_keyword("", "abc"), "");
        assert_eq!(highlight_keyword("Hello", ""), "Hello");
    }

    #[test]
    fn test_extract_lyric_context_middle_line() {
        let lyrics = "第一句歌词\n第二句歌词 肖邦\n第三句歌词\n第四句歌词";
        let kw = "肖邦";
        let context = extract_lyric_context(lyrics, kw).unwrap();
        let expected = "第一句歌词\n第二句歌词 <mark>肖邦</mark>\n第三句歌词";
        assert_eq!(context, expected);
    }

    #[test]
    fn test_extract_lyric_context_first_line_boundary() {
        let lyrics = "第一句歌词 肖邦\n第二句歌词\n第三句歌词";
        let kw = "肖邦";
        let context = extract_lyric_context(lyrics, kw).unwrap();
        let expected = "第一句歌词 <mark>肖邦</mark>\n第二句歌词";
        assert_eq!(context, expected);
    }

    #[test]
    fn test_extract_lyric_context_last_line_boundary() {
        let lyrics = "第一句歌词\n第二句歌词\n第三句歌词 肖邦";
        let kw = "肖邦";
        let context = extract_lyric_context(lyrics, kw).unwrap();
        let expected = "第二句歌词\n第三句歌词 <mark>肖邦</mark>";
        assert_eq!(context, expected);
    }

    #[test]
    fn test_extract_lyric_context_single_line() {
        let lyrics = "唯一的歌词 肖邦";
        let kw = "肖邦";
        let context = extract_lyric_context(lyrics, kw).unwrap();
        assert_eq!(context, "唯一的歌词 <mark>肖邦</mark>");
    }

    #[test]
    fn test_extract_lyric_context_no_match() {
        let lyrics = "第一句歌词\n第二句歌词";
        let kw = "肖邦";
        assert!(extract_lyric_context(lyrics, kw).is_none());
    }

    #[test]
    fn test_extract_lyric_context_empty_inputs() {
        assert!(extract_lyric_context("", "肖邦").is_none());
        assert!(extract_lyric_context("第一句歌词", "").is_none());
    }
}
