/// 不区分大小写的 ASCII 字符串包含匹配
pub fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let needle_bytes = needle.as_bytes();
    let needle_len = needle_bytes.len();

    if needle_len == 0 {
        return true;
    }

    if needle_len > haystack.len() {
        return false;
    }

    haystack
        .as_bytes()
        .windows(needle_len)
        .any(|window| window.eq_ignore_ascii_case(needle_bytes))
}
