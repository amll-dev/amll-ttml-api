//! 缓存控制响应头常量

use axum::http::HeaderValue;

/// 强唯一性歌词获取接口（通过 ID / 文件名获取）的缓存时限（14 天客户端 / 30 天 CDN）
///
/// 指定 ID 和文件名由词库保证不可变，针对某首歌词的修正只会新增歌词而不会修改已有的歌词
pub const EXACT_CACHE_CONTROL: HeaderValue =
    HeaderValue::from_static("public, max-age=1209600, s-maxage=2592000");

/// 模糊搜索和平台 ID 获取接口的缓存时限（3 天客户端 / 7 天 CDN / 1 天 SWR）
pub const WEAK_CACHE_CONTROL: HeaderValue = HeaderValue::from_static(
    "public, max-age=259200, s-maxage=604800, stale-while-revalidate=86400",
);

/// 搜索接口的缓存时限（1 小时客户端 / 2 小时 CDN / 30 分钟 SWR）
pub const SEARCH_CACHE_CONTROL: HeaderValue =
    HeaderValue::from_static("public, max-age=3600, s-maxage=7200, stale-while-revalidate=1800");

/// 404 未找到响应的负缓存时限（1 小时客户端 / 2 小时 CDN）
pub const NOT_FOUND_CACHE_CONTROL: HeaderValue =
    HeaderValue::from_static("public, max-age=3600, s-maxage=7200");

/// 状态与探针接口的缓存时限（禁止缓存）
pub const NO_STORE_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("no-store");
