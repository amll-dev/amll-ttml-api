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
        Request,
        StatusCode,
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
            1_600_000_000,
            &["Test Song One"],
            &["Artist Alpha"],
            &["1001"],
            &["sp1001"],
            &[],
            &[],
        ),
        make_song(
            "test_song_two.ttml",
            1_700_000_000,
            &["Test Song Two"],
            &["Artist Beta"],
            &["1002"],
            &["sp1002"],
            &[],
            &[],
        ),
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

async fn get_body(app: &Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::get(uri)
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router call is infallible");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    (
        status,
        String::from_utf8(bytes.to_vec()).expect("utf-8 body"),
    )
}

fn id_of(filename: &str) -> u64 {
    LyricId::from_filename(filename).get()
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
async fn lyrics_get_not_found_error_shape() {
    let app = test_app().await;
    let (status, body) = get_body(&app, "/v1/lyrics/get?spotifyId=missing").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_snapshot!(body);
}
