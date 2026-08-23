//! 缓存控制响应头常量

use axum::http::HeaderValue;

/// 强唯一性歌词获取接口（通过 ID / 文件名获取）的缓存时限（7 天客户端 / 30 天 CDN / 30 天 SWR）
///
/// 指定 ID 和文件名由词库保证不可变，针对某首歌词的修正只会新增歌词而不会修改已有的歌词，
/// 共享缓存侧因此维持 30 天长时限。
///
/// 客户端侧保留 7 天，因为响应还包含歌词以外的元数据与信息字段，改了 DTO 形状或
/// LRC 解析逻辑之后需要一个召回窗口；7 天短于本仓库线格式变更的实际间隔。不压得更短是因为
/// 取词的耗时结构里连接远重于传输（典型 120ms 握手对 3ms 下载），省掉一次请求的收益比
/// 省掉一次全量重传高两个数量级，而 `ETag` 只能省后者。SWR 再往后延 30 天，
/// 让过期后仍先返回旧副本、后台异步重新验证，握手不进关键路径。
///
/// 注意这个窗口成立的前提是链路上当前没有共享缓存：
/// nginx 目前配置是纯反代，只开了 gzip/brotli，没有配 `proxy_cache`，
/// 所以 `s-maxage` 眼下没有消费者。一旦在前面加了 CDN，客户端 7 天后的重新验证会被 CDN
/// 用自己 30 天内的旧副本挡掉，召回窗口随即失效——那时必须在部署流程里加一步缓存清除，
/// 或者把 `s-maxage` 一并调短。
pub const EXACT_CACHE_CONTROL: HeaderValue = HeaderValue::from_static(
    "public, max-age=604800, s-maxage=2592000, stale-while-revalidate=2592000",
);

/// 模糊搜索和平台 ID 获取接口的缓存时限（7 天客户端 / 7 天 CDN / 7 天 SWR）
///
/// 客户端时限与强唯一档一致（同样由连接成本主导），但**任何**层级都不超过 7 天新鲜期：
/// 匹配结果会随上游新增歌词而变化（近期约 2 首/天），可能出现更好的匹配，
/// 所以不给共享缓存 30 天那一档
pub const WEAK_CACHE_CONTROL: HeaderValue = HeaderValue::from_static(
    "public, max-age=604800, s-maxage=604800, stale-while-revalidate=604800",
);

/// 搜索接口的缓存时限（1 小时客户端 / 2 小时 CDN / 30 分钟 SWR）
pub const SEARCH_CACHE_CONTROL: HeaderValue =
    HeaderValue::from_static("public, max-age=3600, s-maxage=7200, stale-while-revalidate=1800");

/// 404 未找到响应的负缓存时限（1 小时客户端 / 2 小时 CDN）
pub const NOT_FOUND_CACHE_CONTROL: HeaderValue =
    HeaderValue::from_static("public, max-age=3600, s-maxage=7200");

/// 状态与探针接口的缓存时限（禁止缓存）
pub const NO_STORE_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("no-store");
