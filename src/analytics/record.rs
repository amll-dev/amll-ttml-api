//! 单条请求记录，以及从请求 / 响应里提取各字段的规则
//!
//! 所有文本字段都有长度上限，防止有人用超长参数或请求头把缓冲库撑大

use std::{
    net::IpAddr,
    sync::Arc,
    time::{
        Duration,
        SystemTime,
        UNIX_EPOCH,
    },
};

use axum::{
    body::HttpBody,
    extract::{
        MatchedPath,
        Request,
    },
    http::{
        HeaderMap,
        header::{
            CONTENT_LENGTH,
            IF_NONE_MATCH,
            ORIGIN,
            REFERER,
            USER_AGENT,
        },
    },
    response::Response,
};
use hmac::{
    Hmac,
    Mac,
};
use serde_json::{
    Map,
    Value,
    map::Entry,
};
use sha2::Sha256;
use url::{
    Url,
    form_urlencoded,
};

use super::annotation::{
    Annotation,
    MatchKind,
};
use crate::{
    API_PREFIXES,
    utils::string::truncate_utf8,
};

pub type IpHasher = Hmac<Sha256>;

const MAX_USER_AGENT_BYTES: usize = 512;
const MAX_ORIGIN_BYTES: usize = 256;
const MAX_RAW_PATH_BYTES: usize = 256;
const MAX_PARAMS: usize = 16;
const MAX_PARAM_KEY_BYTES: usize = 64;
const MAX_PARAM_VALUE_BYTES: usize = 256;

/// nginx 用 `proxy_set_header X-Real-IP $remote_addr` 覆盖写入，客户端无法伪造。
/// `X-Forwarded-For` 是追加写入，最左段可伪造，不能用
const REAL_IP_HEADER: &str = "x-real-ip";

/// 缓冲库 `requests` 表的一行
#[derive(Debug, Clone)]
pub struct RequestRecord {
    /// 请求到达时刻，UTC epoch 毫秒
    pub ts: i64,
    pub method: String,
    /// `/v1` 或 `/api/v1`，路径不带这两个前缀时为空
    pub prefix: Option<&'static str>,
    /// 去掉前缀的路由模板，例如 `/lrclib/get/{id}`；未匹配到路由时为空
    pub route: Option<String>,
    /// 原始路径，只在未匹配到路由时记录
    pub raw_path: Option<String>,
    /// 解码后的查询参数，JSON 对象；同名参数重复出现时值为数组
    pub params: Option<String>,
    pub status: u16,
    /// 到响应头生成为止的耗时，流式响应体的传输时间不计入
    pub latency_us: i64,
    pub resp_bytes: Option<i64>,
    /// 请求是否带了 `If-None-Match`
    pub conditional: bool,
    pub user_agent: Option<String>,
    pub origin: Option<String>,
    /// 只保留 scheme + host
    pub referer: Option<String>,
    /// 对 `X-Real-IP` 做 HMAC-SHA256 后截断的 64 位值，没有密钥或没有该头时为空
    pub client_id: Option<i64>,
    /// 端口与 git hash，区分蓝绿两个实例
    pub instance: Arc<str>,
    pub hit_count: Option<i64>,
    pub hit_id: Option<i64>,
    pub match_kind: Option<MatchKind>,
    pub norm_query: Option<String>,
}

/// 调用内层服务之前就要取走的请求信息，请求本体随后交给内层
pub struct PendingRecord {
    ts: i64,
    method: String,
    prefix: Option<&'static str>,
    route: Option<String>,
    raw_path: Option<String>,
    params: Option<String>,
    conditional: bool,
    user_agent: Option<String>,
    origin: Option<String>,
    referer: Option<String>,
    client_id: Option<i64>,
}

impl PendingRecord {
    pub fn capture(req: &Request, ip_hasher: Option<&IpHasher>) -> Self {
        let headers = req.headers();
        let path = req.uri().path();
        let matched = req
            .extensions()
            .get::<MatchedPath>()
            .map(MatchedPath::as_str);
        let (prefix, route) = split_route(path, matched);

        Self {
            ts: now_millis(),
            method: req.method().as_str().to_owned(),
            prefix,
            raw_path: route
                .is_none()
                .then(|| truncate_utf8(path, MAX_RAW_PATH_BYTES).to_owned()),
            route,
            params: req.uri().query().and_then(params_json),
            conditional: headers.contains_key(IF_NONE_MATCH),
            user_agent: header_text(headers, USER_AGENT.as_str(), MAX_USER_AGENT_BYTES),
            origin: header_text(headers, ORIGIN.as_str(), MAX_ORIGIN_BYTES),
            referer: header_text(headers, REFERER.as_str(), usize::MAX)
                .and_then(|referer| referer_origin(&referer)),
            client_id: ip_hasher.and_then(|hasher| client_id(headers, hasher)),
        }
    }

    pub fn finish(
        self,
        response: &Response,
        latency: Duration,
        annotation: Option<Annotation>,
        instance: Arc<str>,
    ) -> RequestRecord {
        let annotation = annotation.unwrap_or_default();

        RequestRecord {
            ts: self.ts,
            method: self.method,
            prefix: self.prefix,
            route: self.route,
            raw_path: self.raw_path,
            params: self.params,
            status: response.status().as_u16(),
            latency_us: i64::try_from(latency.as_micros()).unwrap_or(i64::MAX),
            resp_bytes: response_bytes(response),
            conditional: self.conditional,
            user_agent: self.user_agent,
            origin: self.origin,
            referer: self.referer,
            client_id: self.client_id,
            instance,
            hit_count: annotation
                .hit_count
                .map(|n| i64::try_from(n).unwrap_or(i64::MAX)),
            hit_id: annotation.hit_id.map(|id| id.get().cast_signed()),
            match_kind: annotation.match_kind,
            norm_query: annotation.norm_query,
        }
    }
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// 拆出 API 前缀与路由模板
///
/// 路由模板取自 axum 的 `MatchedPath`（嵌套路由下带着前缀，例如 `/v1/lrclib/get/{id}`）；
/// 前缀按原始路径判定，未匹配到路由的请求也能知道走的是哪个前缀
fn split_route(path: &str, matched: Option<&str>) -> (Option<&'static str>, Option<String>) {
    let prefix = API_PREFIXES
        .into_iter()
        .find(|prefix| strip_prefix(path, prefix).is_some());

    let route = matched.map(|matched| {
        prefix
            .and_then(|prefix| strip_prefix(matched, prefix))
            .unwrap_or(matched)
            .to_owned()
    });

    (prefix, route)
}

/// 只在段边界上剥前缀，`/v1x` 不算 `/v1` 下的路径
fn strip_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

/// 把查询串解码成 JSON 对象，同名参数重复出现时收集成数组
///
/// 最多取前 [`MAX_PARAMS`] 对，键和值分别截断；空字符按查询解析器的惯例替换成空格
fn params_json(query: &str) -> Option<String> {
    let mut map = Map::new();

    for (key, value) in form_urlencoded::parse(query.as_bytes()).take(MAX_PARAMS) {
        let key = truncate_utf8(&key, MAX_PARAM_KEY_BYTES).replace('\0', " ");
        let value = Value::String(truncate_utf8(&value, MAX_PARAM_VALUE_BYTES).replace('\0', " "));

        match map.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(value);
            }
            Entry::Occupied(mut entry) => match entry.get_mut() {
                Value::Array(values) => values.push(value),
                existing => {
                    let first = existing.take();
                    *existing = Value::Array(vec![first, value]);
                }
            },
        }
    }

    (!map.is_empty()).then(|| Value::Object(map).to_string())
}

fn header_text(headers: &HeaderMap, name: &str, max_bytes: usize) -> Option<String> {
    let raw = headers.get(name)?;
    let text = String::from_utf8_lossy(raw.as_bytes());
    let text = text.trim();
    (!text.is_empty()).then(|| truncate_utf8(text, max_bytes).to_owned())
}

/// `Referer` 只留 scheme + host（含非默认端口），路径与查询可能带着与统计无关的信息
fn referer_origin(referer: &str) -> Option<String> {
    let origin = Url::parse(referer).ok()?.origin();
    origin
        .is_tuple()
        .then(|| truncate_utf8(&origin.ascii_serialization(), MAX_ORIGIN_BYTES).to_owned())
}

/// 对客户端 IP 做 HMAC-SHA256，取摘要前 8 字节
///
/// 没有密钥无法从结果反推 IP；同一密钥下同一 IP 恒得同一值，可以跨天追踪
fn client_id(headers: &HeaderMap, hasher: &IpHasher) -> Option<i64> {
    let ip: IpAddr = headers
        .get(REAL_IP_HEADER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;

    let mut mac = hasher.clone();
    mac.update(ip.to_string().as_bytes());
    let digest = mac.finalize().into_bytes();

    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    Some(i64::from_be_bytes(bytes))
}

fn response_bytes(response: &Response) -> Option<i64> {
    let exact = response.body().size_hint().exact().or_else(|| {
        response
            .headers()
            .get(CONTENT_LENGTH)?
            .to_str()
            .ok()?
            .parse()
            .ok()
    })?;
    i64::try_from(exact).ok()
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;
    use hmac::KeyInit;

    use super::*;

    fn hasher(key: &[u8]) -> IpHasher {
        IpHasher::new_from_slice(key).unwrap()
    }

    fn headers_with_ip(ip: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(REAL_IP_HEADER, HeaderValue::from_static(ip));
        headers
    }

    #[test]
    fn split_route_strips_either_prefix() {
        assert_eq!(
            split_route("/v1/lrclib/get/42", Some("/v1/lrclib/get/{id}")),
            (Some("/v1"), Some("/lrclib/get/{id}".to_string()))
        );
        assert_eq!(
            split_route("/api/v1/lyrics/get", Some("/api/v1/lyrics/get")),
            (Some("/api/v1"), Some("/lyrics/get".to_string()))
        );
    }

    #[test]
    fn split_route_keeps_prefix_for_unmatched_paths() {
        assert_eq!(split_route("/v1/nope", None), (Some("/v1"), None));
        assert_eq!(split_route("/wp-login.php", None), (None, None));
        // 段边界之外的相似前缀不算
        assert_eq!(split_route("/v1x/lyrics", None), (None, None));
    }

    #[test]
    fn params_json_decodes_and_collects_repeated_keys() {
        let json =
            params_json("musicName=%E6%99%B4%E5%A4%A9&ncmMusicId=1&ncmMusicId=2&x=").unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["musicName"], "晴天");
        assert_eq!(value["ncmMusicId"], serde_json::json!(["1", "2"]));
        assert_eq!(value["x"], "");
    }

    #[test]
    fn params_json_is_bounded() {
        let long_value = "v".repeat(1000);
        let many: Vec<String> = (0..40).map(|i| format!("k{i}={long_value}")).collect();
        let json = params_json(&many.join("&")).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();

        let object = value.as_object().unwrap();
        assert_eq!(object.len(), MAX_PARAMS);
        assert!(
            object
                .values()
                .all(|v| v.as_str().unwrap().len() == MAX_PARAM_VALUE_BYTES)
        );
    }

    #[test]
    fn params_json_replaces_nul_and_skips_empty_query() {
        let json = params_json("q=a%00b").unwrap();
        assert_eq!(json, r#"{"q":"a b"}"#);
        assert_eq!(params_json(""), None);
    }

    #[test]
    fn referer_keeps_only_origin() {
        assert_eq!(
            referer_origin("https://example.com:8443/player?song=1#t=3").as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(
            referer_origin("https://example.com/").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(referer_origin("not a url"), None);
    }

    #[test]
    fn client_id_is_stable_per_key_and_ip() {
        let key_a = hasher(b"key-a");
        let key_b = hasher(b"key-b");

        let first = client_id(&headers_with_ip("203.0.113.7"), &key_a);
        assert!(first.is_some());
        assert_eq!(first, client_id(&headers_with_ip(" 203.0.113.7 "), &key_a));
        assert_ne!(first, client_id(&headers_with_ip("203.0.113.8"), &key_a));
        assert_ne!(first, client_id(&headers_with_ip("203.0.113.7"), &key_b));
    }

    #[test]
    fn client_id_requires_a_valid_real_ip() {
        let key = hasher(b"key");
        assert_eq!(client_id(&HeaderMap::new(), &key), None);
        assert_eq!(client_id(&headers_with_ip("unknown"), &key), None);

        // 只认 X-Real-IP，可伪造的 X-Forwarded-For 不参与
        let mut forwarded = HeaderMap::new();
        forwarded.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.7"));
        assert_eq!(client_id(&forwarded, &key), None);
    }

    #[test]
    fn header_text_trims_and_truncates() {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("  AMLL Player/1.0  "));
        assert_eq!(
            header_text(&headers, USER_AGENT.as_str(), 100).as_deref(),
            Some("AMLL Player/1.0")
        );
        assert_eq!(
            header_text(&headers, USER_AGENT.as_str(), 4).as_deref(),
            Some("AMLL")
        );
        assert_eq!(header_text(&headers, ORIGIN.as_str(), 100), None);
    }
}
