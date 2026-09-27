//! 端到端：经完整路由发请求，停机刷盘后回读缓冲库，核对每一列

use std::{
    sync::Arc,
    time::Duration,
};

use axum::{
    Router,
    body::{
        Body,
        to_bytes,
    },
    http::{
        HeaderMap,
        Method,
        Request,
        StatusCode,
        header::{
            ACCESS_CONTROL_REQUEST_METHOD,
            AUTHORIZATION,
            CACHE_CONTROL,
            CONTENT_DISPOSITION,
            CONTENT_TYPE,
            ETAG,
            IF_NONE_MATCH,
            ORIGIN,
            RANGE,
            REFERER,
            USER_AGENT,
        },
    },
};
use sea_orm::{
    EntityTrait,
    IntoActiveModel,
};
use serde_json::Value;
use sha2::{
    Digest,
    Sha256,
};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tower::ServiceExt;

use super::{
    Analytics,
    AnalyticsConfig,
    AnalyticsTasks,
    BUFFER_FILE,
    buffer::{
        self,
        BufferedRow,
    },
    convert::{
        self,
        ConverterOptions,
        PARQUET_DIR,
    },
    day::DAY_MS,
    record::{
        RequestRecord,
        now_millis,
    },
    retention::RetentionPolicy,
};
use crate::{
    AppState,
    core::{
        LyricId,
        db::entity,
        models::LyricIndexDB,
        test_utils::make_song,
    },
    create_app,
    init_db,
    services::sync_service::build_entity_from_ttml,
};

const SONG_FILE: &str = "test_song_one.ttml";
const SONG_TTML: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="00:01.000" end="00:03.000">Hello World Lyric</p></div></body></tt>"#;

async fn app_with_analytics(config: AnalyticsConfig) -> (Router, AnalyticsTasks) {
    let db_conn = init_db("sqlite::memory:").await.expect("init in-memory db");

    let parsed = ttml_processor::parse_ttml(SONG_TTML).expect("sample ttml must parse");
    let model = build_entity_from_ttml(SONG_FILE, SONG_TTML, &parsed);
    entity::Entity::insert(model.into_active_model())
        .exec(&db_conn)
        .await
        .expect("seed entity row");

    let (analytics, tasks) = Analytics::start(config).await.expect("start analytics");
    let state = AppState::new_with_secret(db_conn, None).with_analytics(analytics);
    state
        .store
        .swap_index(LyricIndexDB::from_entries(vec![make_song(
            SONG_FILE,
            1_689_087_424_000,
            &["Test Song One"],
            &["Artist Alpha"],
            &["1001"],
            &[],
            &[],
            &[],
        )]));

    (create_app(state), tasks)
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap) {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let headers = response.headers().clone();
    to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    (status, headers)
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri)
        .body(Body::empty())
        .expect("build request")
}

async fn read_rows(dir: &TempDir) -> Vec<BufferedRow> {
    let db = buffer::open(&dir.path().join(BUFFER_FILE))
        .await
        .expect("reopen buffer");
    buffer::read_range(&db, i64::MIN, i64::MAX, None, 1_000)
        .await
        .expect("read rows")
}

fn song_id() -> i64 {
    LyricId::from_filename(SONG_FILE).get().cast_signed()
}

#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn records_http_fields_and_business_annotations() {
    let dir = tempfile::tempdir().unwrap();
    let config = AnalyticsConfig::new(dir.path().to_owned(), Some(b"test-key".to_vec()), 3000);
    let instance = config.instance.clone();
    let (app, tasks) = app_with_analytics(config).await;

    // 0：元数据搜索命中，带齐客户端请求头
    let (status, headers) = send(
        &app,
        Request::get("/v1/lyrics/search?musicName=Test+Song+One")
            .header("x-real-ip", "203.0.113.7")
            .header(USER_AGENT, "AMLL Player/1.0")
            .header(ORIGIN, "https://player.example")
            .header(REFERER, "https://player.example/now-playing?song=1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get(ETAG).expect("search is revalidatable").clone();

    // 1：同一请求带 If-None-Match，ETag 中间件降级为 304
    let (status, _) = send(
        &app,
        Request::get("/v1/lyrics/search?musicName=Test+Song+One")
            .header(IF_NONE_MATCH, etag)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);

    // 2：按 ID 取词命中
    let (status, _) = send(&app, get(&format!("/v1/lyrics/get?id={}", song_id()))).await;
    assert_eq!(status, StatusCode::OK);

    // 3：平台 ID 未命中，走 /api/v1 前缀
    let (status, _) = send(&app, get("/api/v1/lyrics/get?ncmMusicId=404404")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 4：lrclib 精确匹配未命中，记下规范化查询
    let (status, _) = send(
        &app,
        get("/v1/lrclib/get?track_name=%E6%99%B4%E5%A4%A9&artist_name=%E5%91%A8%E6%9D%B0%E5%80%AB"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 5：未匹配到路由
    let (status, _) = send(&app, get("/v1/does-not-exist")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 6：CORS 预检
    let (status, _) = send(
        &app,
        Request::builder()
            .method(Method::OPTIONS)
            .uri("/v1/lyrics/search")
            .header(ORIGIN, "https://player.example")
            .header(ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(status.is_success());

    // 7：列表端点
    let (status, _) = send(&app, get("/v1/lyrics/list?pageSize=1")).await;
    assert_eq!(status, StatusCode::OK);

    tasks.shutdown().await;
    let rows = read_rows(&dir).await;
    assert_eq!(rows.len(), 8, "{rows:#?}");

    let search = &rows[0];
    assert_eq!(search.method, "GET");
    assert_eq!(search.prefix.as_deref(), Some("/v1"));
    assert_eq!(search.route.as_deref(), Some("/lyrics/search"));
    assert_eq!(search.raw_path, None);
    assert_eq!(
        search.params.as_deref(),
        Some(r#"{"musicName":"Test Song One"}"#)
    );
    assert_eq!(search.status, 200);
    assert!(search.latency_us >= 0);
    assert!(search.resp_bytes.is_some_and(|bytes| bytes > 0));
    assert!(!search.conditional);
    assert_eq!(search.user_agent.as_deref(), Some("AMLL Player/1.0"));
    assert_eq!(search.origin.as_deref(), Some("https://player.example"));
    assert_eq!(search.referer.as_deref(), Some("https://player.example"));
    assert!(search.client_id.is_some());
    assert_eq!(search.instance, instance);
    assert_eq!(search.hit_count, Some(1));
    assert_eq!(search.hit_id, Some(song_id()));
    assert_eq!(search.match_kind.as_deref(), Some("fuzzy"));
    assert_eq!(search.norm_query, None);

    let revalidated = &rows[1];
    assert_eq!(revalidated.status, 304);
    assert!(revalidated.conditional);
    assert_eq!(revalidated.resp_bytes, Some(0));
    // 没有 X-Real-IP（例如本机健康检查）时客户端标识留空
    assert_eq!(revalidated.client_id, None);

    let by_id = &rows[2];
    assert_eq!(by_id.route.as_deref(), Some("/lyrics/get"));
    assert_eq!(by_id.hit_count, Some(1));
    assert_eq!(by_id.hit_id, Some(song_id()));
    assert_eq!(by_id.match_kind.as_deref(), Some("id"));

    let platform_miss = &rows[3];
    assert_eq!(platform_miss.prefix.as_deref(), Some("/api/v1"));
    assert_eq!(platform_miss.route.as_deref(), Some("/lyrics/get"));
    assert_eq!(platform_miss.status, 404);
    assert_eq!(platform_miss.hit_count, Some(0));
    assert_eq!(platform_miss.hit_id, None);
    assert_eq!(platform_miss.match_kind.as_deref(), Some("platform_id"));
    // 按 ID 查找的需求直接看 params，不生成规范化查询
    assert_eq!(platform_miss.norm_query, None);

    let lrclib_miss = &rows[4];
    assert_eq!(lrclib_miss.route.as_deref(), Some("/lrclib/get"));
    assert_eq!(lrclib_miss.hit_count, Some(0));
    assert_eq!(lrclib_miss.match_kind.as_deref(), Some("fuzzy"));
    assert_eq!(
        lrclib_miss.norm_query.as_deref(),
        Some(r#"{"track":"晴天","artist":"周杰伦"}"#)
    );

    let unmatched = &rows[5];
    assert_eq!(unmatched.prefix.as_deref(), Some("/v1"));
    assert_eq!(unmatched.route, None);
    assert_eq!(unmatched.raw_path.as_deref(), Some("/v1/does-not-exist"));
    assert_eq!(unmatched.hit_count, None);

    let preflight = &rows[6];
    assert_eq!(preflight.method, "OPTIONS");
    assert_eq!(preflight.route.as_deref(), Some("/lyrics/search"));
    assert_eq!(preflight.hit_count, None);

    let list = &rows[7];
    assert_eq!(list.route.as_deref(), Some("/lyrics/list"));
    assert_eq!(list.hit_count, Some(1));
    assert_eq!(list.hit_id, Some(song_id()));
    assert_eq!(list.match_kind, None);
}

#[tokio::test]
async fn missing_ip_key_keeps_recording_without_client_id() {
    let dir = tempfile::tempdir().unwrap();
    let (app, tasks) =
        app_with_analytics(AnalyticsConfig::new(dir.path().to_owned(), None, 3000)).await;

    send(
        &app,
        Request::get("/v1/status")
            .header("x-real-ip", "203.0.113.7")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    tasks.shutdown().await;
    let rows = read_rows(&dir).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].route.as_deref(), Some("/status"));
    assert_eq!(rows[0].client_id, None);
}

#[tokio::test]
async fn low_disk_watermark_discards_records() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = AnalyticsConfig::new(dir.path().to_owned(), None, 3000);
    config.min_free_bytes = u64::MAX;
    let (app, tasks) = app_with_analytics(config).await;

    let (status, _) = send(&app, get("/v1/status")).await;
    assert_eq!(status, StatusCode::OK, "统计停写不能影响请求本身");

    tasks.shutdown().await;
    assert!(read_rows(&dir).await.is_empty());
}

#[tokio::test]
async fn full_channel_drops_and_counts() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = mpsc::channel(1);
    let analytics = Analytics {
        tx,
        ip_hasher: None,
        instance: Arc::from("test"),
        dropped: Arc::default(),
        parquet_dir: Arc::new(dir.path().join(PARQUET_DIR)),
        db: buffer::open(&dir.path().join(BUFFER_FILE)).await.unwrap(),
        secret: None,
    };

    let record = sample_record(0);

    analytics.submit(record.clone());
    analytics.submit(record.clone());
    analytics.submit(record);

    assert_eq!(
        analytics.dropped.load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

fn sample_record(ts: i64) -> RequestRecord {
    RequestRecord {
        ts,
        method: "GET".to_string(),
        prefix: None,
        route: None,
        raw_path: None,
        params: None,
        status: 200,
        latency_us: 0,
        resp_bytes: None,
        conditional: false,
        user_agent: None,
        origin: None,
        referer: None,
        client_id: None,
        instance: Arc::from("test"),
        hit_count: None,
        hit_id: None,
        match_kind: None,
        norm_query: None,
    }
}

const SECRET: &str = "test-analytics-secret";

fn authed(uri: &str) -> Request<Body> {
    Request::get(uri)
        .header(AUTHORIZATION, format!("Bearer {SECRET}"))
        .body(Body::empty())
        .expect("build request")
}

async fn fetch(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    (status, headers, body.to_vec())
}

/// 往缓冲库写几行前天的数据并转换，等文件清单里出现它
///
/// 后台转换任务启动时也会跑一轮，可能抢先认领，所以轮询清单而不是只跑一次
async fn seed_converted_day(dir: &TempDir, app: &Router, rows: i64) -> Value {
    let db = buffer::open(&dir.path().join(BUFFER_FILE)).await.unwrap();
    let day_before_yesterday = now_millis() - 2 * DAY_MS;
    let records: Vec<_> = (0..rows)
        .map(|i| sample_record(day_before_yesterday + i))
        .collect();
    buffer::insert_batch(&db, &records).await.unwrap();

    let options = ConverterOptions {
        db,
        dir: dir.path().to_owned(),
        instance: Arc::from("3000@test"),
        retention: RetentionPolicy {
            retention_days: 90,
            max_total_bytes: u64::MAX,
        },
    };

    for _ in 0..100 {
        convert::run_once(&options, now_millis()).await.unwrap();
        let (status, _, body) = fetch(app, authed("/v1/admin/analytics/files")).await;
        assert_eq!(status, StatusCode::OK);
        let json: Value = serde_json::from_slice(&body).unwrap();
        if let Some(file) = json["data"]["files"].get(0) {
            return file.clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daily file never showed up in the listing");
}

#[tokio::test]
async fn admin_endpoints_list_and_serve_daily_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = AnalyticsConfig::new(dir.path().to_owned(), None, 3000);
    config.secret = Some(SECRET.to_string());
    let (app, tasks) = app_with_analytics(config).await;

    // 鉴权：没带、带错都是 401，且错误响应同样不许缓存
    let (status, headers, _) = fetch(&app, get("/v1/admin/analytics/files")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers[CACHE_CONTROL], "no-store");
    let (status, _, _) = fetch(
        &app,
        Request::get("/v1/admin/analytics/files")
            .header(AUTHORIZATION, "Bearer wrong")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let file = seed_converted_day(&dir, &app, 3).await;
    let name = file["name"].as_str().unwrap().to_owned();
    assert!(name.ends_with(".parquet"));
    assert_eq!(
        file["day"].as_str().unwrap(),
        name.trim_end_matches(".parquet")
    );
    assert_eq!(file["rows"], 3);
    assert_eq!(file["sha256"].as_str().unwrap().len(), 64);

    // 完整下载：内容与清单里的字节数、sha256 对得上
    let uri = format!("/api/v1/admin/analytics/files/{name}");
    let (status, headers, body) = fetch(&app, authed(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[CONTENT_TYPE], "application/vnd.apache.parquet");
    assert_eq!(headers[CACHE_CONTROL], "no-store");
    assert_eq!(headers["x-accel-buffering"], "no");
    assert!(
        headers[CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains(&name)
    );
    assert_eq!(body.len() as u64, file["bytes"].as_u64().unwrap());
    assert_eq!(
        hex::encode(Sha256::digest(&body)),
        file["sha256"].as_str().unwrap()
    );

    // Range：断点续传与 DuckDB 远程读取都靠它
    let (status, _, body) = fetch(
        &app,
        Request::get(&uri)
            .header(AUTHORIZATION, format!("Bearer {SECRET}"))
            .header(RANGE, "bytes=0-3")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, b"PAR1");

    let (status, _, _) = fetch(&app, get(&uri)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 只认严格的每日文件名，也不存在路径穿越
    for bad in ["buffer.db", "2020-01-01.parquet", "..%2Fbuffer.db"] {
        let (status, headers, _) =
            fetch(&app, authed(&format!("/v1/admin/analytics/files/{bad}"))).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{bad}");
        assert_eq!(headers[CACHE_CONTROL], "no-store", "{bad}");
    }

    tasks.shutdown().await;
}

#[tokio::test]
async fn admin_endpoints_without_analytics_or_secret() {
    // 统计未启用：端点等同于不存在
    let db_conn = init_db("sqlite::memory:").await.unwrap();
    let app = create_app(AppState::new_with_secret(db_conn, None));
    let (status, _, _) = fetch(&app, authed("/v1/admin/analytics/files")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 启用了但没配密钥：服务端配置错误，与 webhook 一致
    let dir = tempfile::tempdir().unwrap();
    let (app, tasks) =
        app_with_analytics(AnalyticsConfig::new(dir.path().to_owned(), None, 3000)).await;
    let (status, _, _) = fetch(&app, authed("/v1/admin/analytics/files")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    tasks.shutdown().await;
}
