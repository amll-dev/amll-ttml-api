//! 端到端：经完整路由发请求，停机刷盘后回读缓冲库，核对每一列

use std::sync::Arc;

use axum::{
    Router,
    body::{
        Body,
        to_bytes,
    },
    http::{
        Method,
        Request,
        StatusCode,
        header::{
            ACCESS_CONTROL_REQUEST_METHOD,
            ETAG,
            IF_NONE_MATCH,
            ORIGIN,
            REFERER,
            USER_AGENT,
        },
    },
};
use sea_orm::{
    DatabaseBackend,
    EntityTrait,
    FromQueryResult,
    IntoActiveModel,
    Statement,
};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tower::ServiceExt;

use super::{
    Analytics,
    AnalyticsConfig,
    AnalyticsWriter,
    BUFFER_FILE,
    buffer,
    record::RequestRecord,
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

#[derive(Debug, FromQueryResult)]
struct Row {
    method: String,
    prefix: Option<String>,
    route: Option<String>,
    raw_path: Option<String>,
    params: Option<String>,
    status: i32,
    latency_us: i64,
    resp_bytes: Option<i64>,
    conditional: bool,
    user_agent: Option<String>,
    origin: Option<String>,
    referer: Option<String>,
    client_id: Option<i64>,
    instance: String,
    hit_count: Option<i64>,
    hit_id: Option<i64>,
    match_kind: Option<String>,
    norm_query: Option<String>,
}

async fn app_with_analytics(config: AnalyticsConfig) -> (Router, AnalyticsWriter) {
    let db_conn = init_db("sqlite::memory:").await.expect("init in-memory db");

    let parsed = ttml_processor::parse_ttml(SONG_TTML).expect("sample ttml must parse");
    let model = build_entity_from_ttml(SONG_FILE, SONG_TTML, &parsed);
    entity::Entity::insert(model.into_active_model())
        .exec(&db_conn)
        .await
        .expect("seed entity row");

    let (analytics, writer) = Analytics::start(config).await.expect("start analytics");
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

    (create_app(state), writer)
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap) {
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

async fn read_rows(dir: &TempDir) -> Vec<Row> {
    let db = buffer::open(&dir.path().join(BUFFER_FILE))
        .await
        .expect("reopen buffer");
    Row::find_by_statement(Statement::from_string(
        DatabaseBackend::Sqlite,
        "SELECT * FROM requests ORDER BY rowid",
    ))
    .all(&db)
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
    let (app, writer) = app_with_analytics(config).await;

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

    writer.shutdown().await;
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
    let (app, writer) =
        app_with_analytics(AnalyticsConfig::new(dir.path().to_owned(), None, 3000)).await;

    send(
        &app,
        Request::get("/v1/status")
            .header("x-real-ip", "203.0.113.7")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    writer.shutdown().await;
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
    let (app, writer) = app_with_analytics(config).await;

    let (status, _) = send(&app, get("/v1/status")).await;
    assert_eq!(status, StatusCode::OK, "统计停写不能影响请求本身");

    writer.shutdown().await;
    assert!(read_rows(&dir).await.is_empty());
}

#[tokio::test]
async fn full_channel_drops_and_counts() {
    let (tx, _rx) = mpsc::channel(1);
    let analytics = Analytics {
        tx,
        ip_hasher: None,
        instance: Arc::from("test"),
        dropped: Arc::default(),
    };

    let record = RequestRecord {
        ts: 0,
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
    };

    analytics.submit(record.clone());
    analytics.submit(record.clone());
    analytics.submit(record);

    assert_eq!(
        analytics.dropped.load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}
