//! 条件请求支持：为可缓存响应生成 `ETag`，并在 `If-None-Match` 命中时返回 304
//!
//! `ETag` 取响应体的哈希，因为上游元数据重算、LRC 解析器修正、DTO 结构调整都会改变响应体，使用
//! body 哈希可以避免算漏某个维度

use axum::{
    body::{
        Body,
        to_bytes,
    },
    extract::Request,
    http::{
        HeaderValue,
        Method,
        StatusCode,
        header::{
            CACHE_CONTROL,
            CONTENT_LENGTH,
            CONTENT_TYPE,
            ETAG,
            IF_NONE_MATCH,
        },
    },
    middleware::Next,
    response::{
        IntoResponse,
        Response,
    },
};
use sha2::{
    Digest,
    Sha256,
};
use tracing::warn;

use crate::core::error::AppError;

/// 响应体缓冲上限，正常响应远小于此值
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// `ETag` 摘要取 SHA-256 的前 16 字节，128 位足以排除碰撞
const DIGEST_BYTES: usize = 16;

/// 去掉弱校验前缀，用于 [RFC 9110 §8.8.3.2](https://www.rfc-editor.org/rfc/rfc9110.html#section-8.8.3.2) 的弱比较
///
/// nginx 压缩响应时会把强 `ETag` 改写成弱 `ETag`，若只做字面比较，会导致客户端回传的 `W/"..."`
/// 永远匹配不上本服务生成的标签。因此本服务直接生成弱 `ETag`，且比较前两侧都归一化
fn strip_weak(tag: &str) -> &str {
    tag.strip_prefix("W/").unwrap_or(tag).trim()
}

/// `If-None-Match` 是否命中给定 `ETag`
///
/// 支持 `*` 通配与逗号分隔的多标签列表
fn if_none_match_hit(header: &HeaderValue, etag: &str) -> bool {
    let Ok(raw) = header.to_str() else {
        return false;
    };

    let raw = raw.trim();
    if raw == "*" {
        return true;
    }

    let target = strip_weak(etag);
    raw.split(',')
        .any(|candidate| strip_weak(candidate.trim()) == target)
}

/// 任一 `If-None-Match` 头行是否命中给定 `ETag`
///
/// [RFC 9110 §5.2](https://www.rfc-editor.org/rfc/rfc9110.html#section-5.2) 允许同名头以多个
/// 头行的形式重复出现，语义等价于把各行用逗号拼接。有的客户端库是 append 而不是 join，
/// 只读第一行会漏掉后面几行里的命中，白发一次全量 body
fn any_if_none_match_hit(candidates: &[HeaderValue], etag: &str) -> bool {
    candidates
        .iter()
        .any(|candidate| if_none_match_hit(candidate, etag))
}

/// 响应是否参与条件请求
///
/// 只有带 `public` 的 200 响应才打 `ETag`：`no-store` 的探针端点不需要重新验证，
/// 负缓存的 404 也没有可校验的表示。判据跟着 [`super::cache`] 的分档走，
/// 新增端点只要选定了缓存档位就自动获得 `ETag`
fn is_revalidatable(response: &Response) -> bool {
    response.status() == StatusCode::OK
        && response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("public"))
}

/// 为可缓存响应附加 `ETag`，并在 `If-None-Match` 命中时降级为 304
pub async fn apply(req: Request, next: Next) -> Response {
    let is_get = req.method() == Method::GET;
    let if_none_match: Vec<HeaderValue> = req
        .headers()
        .get_all(IF_NONE_MATCH)
        .iter()
        .cloned()
        .collect();

    let response = next.run(req).await;

    if !is_get || !is_revalidatable(&response) {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(e) => {
            return AppError::InternalServerError(format!(
                "Failed to buffer response body for ETag: {e}"
            ))
            .into_response();
        }
    };

    let digest = Sha256::digest(&bytes);
    let etag = format!("W/\"{}\"", hex::encode(&digest[..DIGEST_BYTES]));

    match HeaderValue::from_str(&etag) {
        Ok(value) => parts.headers.insert(ETAG, value),
        Err(e) => {
            warn!("Generated ETag is not a valid header value: {e}");
            return Response::from_parts(parts, Body::from(bytes));
        }
    };

    if any_if_none_match_hit(&if_none_match, &etag) {
        // 304 必须保留 ETag 与 Cache-Control。后者用于刷新客户端已存副本的新鲜度，
        // 缺失会导致下次请求立刻又要重新验证。Vary 由 nginx 在更外层附加
        parts.status = StatusCode::NOT_MODIFIED;
        parts.headers.remove(CONTENT_TYPE);
        parts.headers.remove(CONTENT_LENGTH);
        return Response::from_parts(parts, Body::empty());
    }

    Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use axum::http::{
        HeaderMap,
        HeaderValue,
        header::IF_NONE_MATCH,
    };

    use super::{
        any_if_none_match_hit,
        if_none_match_hit,
        strip_weak,
    };

    const TAG: &str = "W/\"0123456789abcdef\"";

    fn appended(lines: &[&'static str]) -> Vec<HeaderValue> {
        let mut headers = HeaderMap::new();
        for line in lines {
            headers.append(IF_NONE_MATCH, HeaderValue::from_static(line));
        }
        headers.get_all(IF_NONE_MATCH).iter().cloned().collect()
    }

    #[test]
    fn strip_weak_removes_prefix_only_when_present() {
        assert_eq!(strip_weak("W/\"abc\""), "\"abc\"");
        assert_eq!(strip_weak("\"abc\""), "\"abc\"");
    }

    #[test]
    fn weak_and_strong_forms_compare_equal() {
        let strong = HeaderValue::from_static("\"0123456789abcdef\"");
        assert!(if_none_match_hit(&strong, TAG));
    }

    #[test]
    fn identical_weak_form_hits() {
        let same = HeaderValue::from_static("W/\"0123456789abcdef\"");
        assert!(if_none_match_hit(&same, TAG));
    }

    #[test]
    fn wildcard_hits() {
        let wildcard = HeaderValue::from_static("*");
        assert!(if_none_match_hit(&wildcard, TAG));
    }

    #[test]
    fn tag_list_hits_when_any_member_matches() {
        let list = HeaderValue::from_static("W/\"deadbeef\", W/\"0123456789abcdef\"");
        assert!(if_none_match_hit(&list, TAG));
    }

    #[test]
    fn unrelated_tag_misses() {
        let other = HeaderValue::from_static("W/\"deadbeef\"");
        assert!(!if_none_match_hit(&other, TAG));
    }

    #[test]
    fn non_ascii_header_misses_without_panicking() {
        let invalid = HeaderValue::from_bytes(&[0xff, 0xfe]).expect("opaque header value");
        assert!(!if_none_match_hit(&invalid, TAG));
    }

    #[test]
    fn absent_header_misses() {
        assert!(!any_if_none_match_hit(&appended(&[]), TAG));
    }

    #[test]
    fn repeated_header_lines_hit_on_any_line() {
        let lines = appended(&["W/\"deadbeef\"", "W/\"0123456789abcdef\""]);
        assert_eq!(lines.len(), 2);
        assert!(any_if_none_match_hit(&lines, TAG));
    }

    #[test]
    fn repeated_header_lines_miss_when_none_match() {
        let lines = appended(&["W/\"deadbeef\"", "W/\"cafebabe\""]);
        assert!(!any_if_none_match_hit(&lines, TAG));
    }

    #[test]
    fn wildcard_on_a_later_line_hits() {
        assert!(any_if_none_match_hit(
            &appended(&["W/\"deadbeef\"", "*"]),
            TAG
        ));
    }
}
