//! 线格式金样测试（wire-format golden tests）。
//!
//! 逐字节锁定各数据端点与错误路径的对外 JSON 形状——信封、分页元数据、
//! 字段顺序、可选字段的缺席行为。使快照失败的改动即对客户端的破坏性变更，
//! 需要有明确的版本化决策。

use axum::{
    Router,
    body::{
        Body,
        to_bytes,
    },
    http::{
        HeaderMap,
        Request,
        StatusCode,
        header::{
            ACCESS_CONTROL_EXPOSE_HEADERS,
            CACHE_CONTROL,
            CONTENT_TYPE,
            ETAG,
            IF_NONE_MATCH,
            ORIGIN,
        },
    },
};
use insta::assert_snapshot;
use sea_orm::{
    EntityTrait,
    IntoActiveModel,
};
use tower::ServiceExt;

use crate::{
    AppState,
    api::shared::cache::{
        EXACT_CACHE_CONTROL,
        NO_STORE_CACHE_CONTROL,
        NOT_FOUND_CACHE_CONTROL,
        SEARCH_CACHE_CONTROL,
        WEAK_CACHE_CONTROL,
    },
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

const TTML_ONE: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="00:01.000" end="00:03.000">Hello World Lyric</p></div></body></tt>"#;
const TTML_TWO: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="00:02.000" end="00:04.000">Second Song Lyric</p></div></body></tt>"#;

async fn test_app() -> Router {
    let db_conn = init_db("sqlite::memory:").await.expect("init in-memory db");

    let index = LyricIndexDB::from_entries(vec![
        make_song(
            "test_song_one.ttml",
            1_689_087_424_000,
            &["Test Song One"],
            &["Artist Alpha"],
            &["1001"],
            &["sp1001"],
            &["108002475"],
            &["alpha_author"],
        )
        .with_isrcs(&["USRC17607839"]),
        make_song(
            "test_song_two.ttml",
            1_787_382_687_321,
            &["Test Song Two"],
            &["Artist Beta"],
            &["1002"],
            &["sp1002"],
            &["330521690"],
            &["beta_author"],
        )
        .with_qq_music_ids(&["qq1002"]),
    ]);

    for (filename, raw) in [
        ("test_song_one.ttml", TTML_ONE),
        ("test_song_two.ttml", TTML_TWO),
    ] {
        let parsed = ttml_processor::parse_ttml(raw).expect("sample ttml must parse");
        let model = build_entity_from_ttml(filename, raw, &parsed);
        entity::Entity::insert(model.into_active_model())
            .exec(&db_conn)
            .await
            .expect("seed entity row");
    }

    let state = AppState::new_with_secret(db_conn, None);
    state.store.swap_index(index);
    create_app(state)
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("router call is infallible");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    (
        status,
        headers,
        String::from_utf8(bytes.to_vec()).expect("utf-8 body"),
    )
}

async fn get_response(app: &Router, uri: &str) -> (StatusCode, HeaderMap, String) {
    send(
        app,
        Request::get(uri)
            .body(Body::empty())
            .expect("build request"),
    )
    .await
}

async fn conditional_get(
    app: &Router,
    uri: &str,
    if_none_match: &str,
) -> (StatusCode, HeaderMap, String) {
    send(
        app,
        Request::get(uri)
            .header(IF_NONE_MATCH, if_none_match)
            .body(Body::empty())
            .expect("build request"),
    )
    .await
}

fn etag_of(headers: &HeaderMap) -> String {
    headers
        .get(ETAG)
        .expect("response carries an ETag")
        .to_str()
        .expect("ETag is ascii")
        .to_owned()
}

async fn get_body(app: &Router, uri: &str) -> (StatusCode, String) {
    let (status, _, body) = get_response(app, uri).await;
    (status, body)
}

fn id_of(filename: &str) -> u64 {
    LyricId::from_filename(filename).get()
}

fn filenames_in(body: &str) -> Vec<&str> {
    const KEY: &str = r#""filename":""#;

    body.match_indices(KEY)
        .map(|(start, _)| {
            let rest = &body[start + KEY.len()..];
            let end = rest.find('"').expect("filename is a closed json string");
            &rest[..end]
        })
        .collect()
}

#[tokio::test]
async fn lyrics_get_returns_enveloped_item_with_ttml() {
    let app = test_app().await;
    let (status, body) = get_body(&app, "/v1/lyrics/get?spotifyId=sp1001").await;

    assert_eq!(status, StatusCode::OK);
    assert_snapshot!(body);
}

#[tokio::test]
async fn lyrics_search_by_metadata_returns_envelope_and_pagination() {
    let app = test_app().await;
    let (status, body) = get_body(&app, "/v1/lyrics/search?musicName=Test+Song+One").await;

    assert_eq!(status, StatusCode::OK);
    assert_snapshot!(body);
}

#[tokio::test]
async fn lyrics_search_by_lyric_text_returns_highlighted_snippet() {
    let app = test_app().await;
    let (status, body) = get_body(&app, "/v1/lyrics/search?lyricText=Hello").await;

    assert_eq!(status, StatusCode::OK);
    assert_snapshot!(body);
}

#[tokio::test]
async fn lrclib_search_returns_bare_array_sorted_by_recency() {
    let app = test_app().await;
    let (status, body) = get_body(&app, "/v1/lrclib/search?q=Artist").await;

    assert_eq!(status, StatusCode::OK);
    assert_snapshot!(body);
}

#[tokio::test]
async fn lrclib_get_by_fields_returns_single_item() {
    let app = test_app().await;
    let (status, body) = get_body(
        &app,
        "/v1/lrclib/get?track_name=Test+Song+One&artist_name=Artist+Alpha",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_snapshot!(body);
}

#[tokio::test]
async fn lrclib_get_by_id_returns_single_item() {
    let app = test_app().await;
    let (status, body) = get_body(
        &app,
        &format!("/v1/lrclib/get/{}", id_of("test_song_two.ttml")),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_snapshot!(body);
}

#[tokio::test]
async fn lyrics_list_returns_newest_first_with_pagination() {
    let app = test_app().await;
    let (status, body) = get_body(&app, "/v1/lyrics/list").await;

    assert_eq!(status, StatusCode::OK);
    assert_snapshot!(body);
}

#[tokio::test]
async fn lyrics_list_accepts_pagination_without_search_params() {
    let app = test_app().await;

    let (status, body) = get_body(&app, "/v1/lyrics/list?page=2&pageSize=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("test_song_one.ttml"), "{body}");

    let (status, _) = get_body(&app, "/v1/lyrics/list?pageSize=101").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = get_body(&app, "/v1/lyrics/list?page=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn lyrics_list_accepts_sort_and_order() {
    let app = test_app().await;

    let (status, body) = get_body(&app, "/v1/lyrics/list?sort=musicName&order=asc").await;
    assert_eq!(status, StatusCode::OK);
    let first = body.find("test_song_one.ttml").unwrap();
    let second = body.find("test_song_two.ttml").unwrap();
    assert!(
        first < second,
        "musicName asc should order Song One before Song Two"
    );

    let (status, _) = get_body(&app, "/v1/lyrics/list?sort=unknown").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = get_body(&app, "/v1/lyrics/list?sort=artistName").await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = get_body(&app, "/v1/lyrics/list?sort=albumName").await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = get_body(&app, "/v1/lyrics/list?order=up").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn lyrics_list_filtered_returns_shrunken_pagination() {
    let app = test_app().await;
    let (status, body) = get_body(&app, "/v1/lyrics/list?hasId=isrc").await;

    assert_eq!(status, StatusCode::OK);
    assert_snapshot!(body);
}

#[tokio::test]
async fn lyrics_list_filtered_to_nothing_is_an_empty_page_not_a_404() {
    let app = test_app().await;
    let (status, body) = get_body(
        &app,
        "/v1/lyrics/list?hasId=isrc&authorUsername=beta_author",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_snapshot!(body);
}

#[tokio::test]
async fn lyrics_list_filters_are_conjunctive() {
    const ONE: &str = "test_song_one.ttml";
    const TWO: &str = "test_song_two.ttml";

    let app = test_app().await;

    // 过滤维度之间、以及 `hasId` / `missingId` 内部的多个取值之间，全都是 AND
    for (query, expected) in [
        ("authorId=108002475", vec![ONE]),
        ("authorUsername=beta_author", vec![TWO]),
        ("authorId=108002475&authorUsername=beta_author", vec![]),
        ("authorId=nobody", vec![]),
        ("hasId=isrc", vec![ONE]),
        ("missingId=isrc", vec![TWO]),
        ("hasId=ncmMusicId,qqMusicId", vec![TWO]),
        ("hasId=ncmMusicId&hasId=isrc", vec![ONE]),
        ("hasId=spotifyId&missingId=qqMusicId,isrc", vec![]),
        ("hasId=ncmMusicId&missingId=isrc", vec![TWO]),
        // 默认排序仍是 createdAt desc，Two 比 One 新
        ("hasId=ncmMusicId,spotifyId", vec![TWO, ONE]),
        // 空取值等同未传，退化为全量列表
        ("hasId=&missingId=,+,", vec![TWO, ONE]),
    ] {
        let (status, body) = get_body(&app, &format!("/v1/lyrics/list?{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query} => {body}");
        assert_eq!(filenames_in(&body), expected, "{query} => {body}");
        assert!(
            body.contains(&format!("\"total\":{}", expected.len())),
            "{query} => {body}"
        );
    }
}

#[tokio::test]
async fn lyrics_list_rejects_bad_filter_values() {
    let app = test_app().await;

    for query in [
        "hasId=ncm",
        "missingId=isrcs",
        "hasId=ncmMusicId,bogus",
        // 同一个标识既要求有又要求缺，只可能是拼错了参数
        "hasId=isrc&missingId=isrc",
        "hasId=ncmMusicId,isrc&missingId=isrc",
    ] {
        let (status, body) = get_body(&app, &format!("/v1/lyrics/list?{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query} => {body}");
    }
}

#[tokio::test]
async fn lyrics_list_cursor_pagination_walks_whole_set() {
    let app = test_app().await;

    // 第 1 页：offset 模式 pageSize=1，应当包含 test_song_two.ttml 与 nextCursor
    let (status, body) = get_body(&app, "/v1/lyrics/list?pageSize=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("test_song_two.ttml"), "{body}");
    assert!(body.contains(r#""hasMore":true"#), "{body}");
    assert!(body.contains(r#""nextCursor":"#), "{body}");

    // 提取 nextCursor
    let key = r#""nextCursor":""#;
    let start = body.find(key).expect("nextCursor 应存在") + key.len();
    let end = body[start..].find('"').expect("closed quote") + start;
    let next_cursor = &body[start..end];

    // 第 2 页：通过 cursor 访问下一页
    let (status, body2) = get_body(&app, &format!("/v1/lyrics/list?pageSize=1&cursor={next_cursor}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body2.contains("test_song_one.ttml"), "{body2}");
    assert!(body2.contains(r#""hasMore":false"#), "{body2}");
    assert!(!body2.contains("nextCursor"), "{body2}");
    // 游标模式下 page 与 totalPages 被省略
    assert!(!body2.contains(r#""page":"#), "{body2}");
    assert!(!body2.contains(r#""totalPages":"#), "{body2}");

    // 通过 before 别名访问同样生效
    let (status, body_before) = get_body(&app, &format!("/v1/lyrics/list?pageSize=1&before={next_cursor}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body_before.contains("test_song_one.ttml"), "{body_before}");
}

#[tokio::test]
async fn lyrics_list_since_and_until_filters() {
    let app = test_app().await;

    // test_song_two(1787382687321) > 1700000000000 > test_song_one(1689087424000)
    let (status, body) = get_body(&app, "/v1/lyrics/list?since=1700000000000").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filenames_in(&body), vec!["test_song_two.ttml"]);
    assert!(body.contains(r#""total":1"#), "{body}");

    let (status, body) = get_body(&app, "/v1/lyrics/list?until=1700000000000").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filenames_in(&body), vec!["test_song_one.ttml"]);
    assert!(body.contains(r#""total":1"#), "{body}");

    let (status, body) = get_body(&app, "/v1/lyrics/list?since=1800000000000").await;
    assert_eq!(status, StatusCode::OK);
    assert!(filenames_in(&body).is_empty());
    assert!(body.contains(r#""total":0"#), "{body}");
}

#[tokio::test]
async fn lyrics_list_cursor_rejects_conflicts_and_bad_inputs() {
    let app = test_app().await;

    for query in [
        // page 与 cursor 互斥
        "page=1&cursor=1787382687321_1002",
        "page=2&before=1787382687321_1002",
        // cursor 与 before 冲突
        "cursor=1787382687321_1002&before=1689087424000_1001",
        // cursor 与非 createdAt 排序互斥
        "sort=artistName&cursor=1787382687321_1002",
        "sort=musicName&before=1787382687321_1002",
        // 非法 cursor 格式
        "cursor=invalid",
        "cursor=1787382687321_",
        // since > until
        "since=2000000000000&until=1000000000000",
        // 非数字 since / until
        "since=abc",
        "until=xyz",
    ] {
        let (status, body) = get_body(&app, &format!("/v1/lyrics/list?{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query} => {body}");
    }
}

#[tokio::test]
async fn lyrics_get_not_found_error_shape() {
    let app = test_app().await;
    let (status, body) = get_body(&app, "/v1/lyrics/get?spotifyId=missing").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_snapshot!(body);
}

#[tokio::test]
async fn cache_control_exact_and_weak_headers() {
    let app = test_app().await;

    let id_1 = id_of("test_song_one.ttml");
    let (status, headers, _) = get_response(&app, &format!("/v1/lyrics/get?id={id_1}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), EXACT_CACHE_CONTROL);

    let (status, headers, _) =
        get_response(&app, "/v1/lyrics/get?filename=test_song_one.ttml").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), EXACT_CACHE_CONTROL);

    let (status, headers, _) = get_response(&app, &format!("/v1/lrclib/get/{id_1}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), EXACT_CACHE_CONTROL);

    let (status, headers, _) = get_response(&app, "/v1/lyrics/get?spotifyId=sp1001").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), WEAK_CACHE_CONTROL);

    let (status, headers, _) = get_response(
        &app,
        "/v1/lrclib/get?track_name=Test+Song+One&artist_name=Artist+Alpha",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), WEAK_CACHE_CONTROL);

    let (status, headers, _) = get_response(&app, "/v1/lyrics/get?spotifyId=missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), NOT_FOUND_CACHE_CONTROL);

    let (status, headers, _) = get_response(&app, "/v1/nonexistent_route").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), NOT_FOUND_CACHE_CONTROL);

    let (status, headers, _) =
        get_response(&app, "/v1/lyrics/search?musicName=Test+Song+One").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), SEARCH_CACHE_CONTROL);

    let (status, headers, _) = get_response(&app, "/v1/lrclib/search?q=Artist").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), SEARCH_CACHE_CONTROL);

    let (status, headers, _) = get_response(&app, "/v1/lyrics/list").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), SEARCH_CACHE_CONTROL);

    let (status, headers, _) = get_response(&app, "/v1/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), NO_STORE_CACHE_CONTROL);

    let (status, headers, _) = get_response(&app, "/v1/version").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), NO_STORE_CACHE_CONTROL);
}

#[tokio::test]
async fn etag_present_on_every_cacheable_endpoint() {
    let app = test_app().await;
    let id_1 = id_of("test_song_one.ttml");

    for uri in [
        format!("/v1/lyrics/get?id={id_1}"),
        "/v1/lyrics/get?filename=test_song_one.ttml".to_owned(),
        format!("/v1/lrclib/get/{id_1}"),
        "/v1/lyrics/get?spotifyId=sp1001".to_owned(),
        "/v1/lrclib/get?track_name=Test+Song+One&artist_name=Artist+Alpha".to_owned(),
        "/v1/lyrics/search?musicName=Test+Song+One".to_owned(),
        "/v1/lrclib/search?q=Artist".to_owned(),
        "/v1/lyrics/list".to_owned(),
    ] {
        let (status, headers, _) = get_response(&app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");

        let etag = etag_of(&headers);
        assert!(etag.starts_with("W/\""), "{uri} produced {etag}");
        // 弱前缀 + 引号包裹的 32 位 hex（SHA-256 前 16 字节）
        assert_eq!(etag.len(), 36, "{uri} produced {etag}");
    }
}

#[tokio::test]
async fn etag_is_scoped_per_resource() {
    let app = test_app().await;

    let (_, one, _) = get_response(
        &app,
        &format!("/v1/lyrics/get?id={}", id_of("test_song_one.ttml")),
    )
    .await;
    let (_, two, _) = get_response(
        &app,
        &format!("/v1/lyrics/get?id={}", id_of("test_song_two.ttml")),
    )
    .await;

    assert_ne!(etag_of(&one), etag_of(&two));
}

#[tokio::test]
async fn etag_is_stable_across_repeated_requests() {
    let app = test_app().await;
    let uri = format!("/v1/lyrics/get?id={}", id_of("test_song_one.ttml"));

    let (_, first, _) = get_response(&app, &uri).await;
    let (_, second, _) = get_response(&app, &uri).await;

    assert_eq!(etag_of(&first), etag_of(&second));
}

#[tokio::test]
async fn conditional_get_returns_304_preserving_validator_and_freshness() {
    let app = test_app().await;
    let uri = format!("/v1/lyrics/get?id={}", id_of("test_song_one.ttml"));

    let (status, headers, body) = get_response(&app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Hello World Lyric"));
    let etag = etag_of(&headers);

    let (status, headers, body) = conditional_get(&app, &uri, &etag).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
    // 304 必须带回校验器与新鲜度，否则客户端下次立刻又要重新验证
    assert_eq!(etag_of(&headers), etag);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), EXACT_CACHE_CONTROL);
    assert!(headers.get(CONTENT_TYPE).is_none());
}

/// nginx 压缩响应时会在强弱形式之间改写 `ETag`，两种形式都必须命中
#[tokio::test]
async fn conditional_get_accepts_strong_form_of_weak_etag() {
    let app = test_app().await;
    let uri = "/v1/lyrics/search?musicName=Test+Song+One";

    let (_, headers, _) = get_response(&app, uri).await;
    let etag = etag_of(&headers);
    let strong = etag.strip_prefix("W/").expect("generated etag is weak");

    let (status, _, body) = conditional_get(&app, uri, strong).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
}

#[tokio::test]
async fn conditional_get_supports_wildcard_and_tag_lists() {
    let app = test_app().await;
    let uri = format!("/v1/lrclib/get/{}", id_of("test_song_one.ttml"));

    let (_, headers, _) = get_response(&app, &uri).await;
    let etag = etag_of(&headers);

    let (status, _, _) = conditional_get(&app, &uri, "*").await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);

    let list = format!("W/\"00000000000000000000000000000000\", {etag}");
    let (status, _, _) = conditional_get(&app, &uri, &list).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
}

/// 有的客户端库把多个校验器 append 成多个同名头行而不是拼成一行，
/// 只读第一行会漏掉后面的命中
#[tokio::test]
async fn conditional_get_accepts_repeated_if_none_match_lines() {
    let app = test_app().await;
    let uri = format!("/v1/lyrics/get?id={}", id_of("test_song_one.ttml"));

    let (_, headers, _) = get_response(&app, &uri).await;
    let etag = etag_of(&headers);

    let (status, _, body) = send(
        &app,
        Request::get(&uri)
            .header(IF_NONE_MATCH, "W/\"00000000000000000000000000000000\"")
            .header(IF_NONE_MATCH, &etag)
            .body(Body::empty())
            .expect("build request"),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
}

/// `ETag` 不在 CORS 安全列表内，不显式暴露的话跨域 JS 读不到它，
/// 也就永远无法自己回传 `If-None-Match`
#[tokio::test]
async fn etag_is_exposed_to_cross_origin_clients() {
    let app = test_app().await;
    let uri = format!("/v1/lyrics/get?id={}", id_of("test_song_one.ttml"));

    let (status, headers, _) = send(
        &app,
        Request::get(&uri)
            .header(ORIGIN, "https://amll.dev")
            .body(Body::empty())
            .expect("build request"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let exposed = headers
        .get(ACCESS_CONTROL_EXPOSE_HEADERS)
        .expect("CORS exposes headers")
        .to_str()
        .expect("header is ascii")
        .to_ascii_lowercase();
    assert!(
        exposed.split(',').any(|name| name.trim() == "etag"),
        "{exposed}"
    );
}

#[tokio::test]
async fn stale_etag_returns_full_body() {
    let app = test_app().await;
    let uri = format!("/v1/lyrics/get?id={}", id_of("test_song_one.ttml"));

    let (status, headers, body) =
        conditional_get(&app, &uri, "W/\"00000000000000000000000000000000\"").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Hello World Lyric"));
    assert!(!etag_of(&headers).is_empty());
}

#[tokio::test]
async fn uncacheable_and_error_responses_carry_no_etag() {
    let app = test_app().await;

    // no-store 的探针端点不参与重新验证
    let (status, headers, _) = get_response(&app, "/v1/status").await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(ETAG).is_none());

    // 负缓存的 404 没有可校验的表示
    let (status, headers, _) = get_response(&app, "/v1/lyrics/get?spotifyId=missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(headers.get(ETAG).is_none());

    let (status, headers, _) = get_response(&app, "/v1/nonexistent_route").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(headers.get(ETAG).is_none());

    // 400 同样不打校验器
    let (status, headers, _) = get_response(&app, "/v1/lyrics/get?id=1&format=lrc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(headers.get(ETAG).is_none());
}

/// axum 在路由内部丢弃 HEAD 的响应体，中间件在外层只能观测到空体，
/// 因此有意不为 HEAD 生成 `ETag`，避免发出与 GET 不一致的校验器
#[tokio::test]
async fn head_request_carries_no_etag() {
    let app = test_app().await;
    let uri = format!("/v1/lyrics/get?id={}", id_of("test_song_one.ttml"));

    let (status, headers, body) = send(
        &app,
        Request::head(&uri)
            .body(Body::empty())
            .expect("build request"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), EXACT_CACHE_CONTROL);
    assert!(headers.get(ETAG).is_none());
}
