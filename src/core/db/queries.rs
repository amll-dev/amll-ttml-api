//! SQLite 查询层
//!
//! 负责 FTS 全文检索、读取原始 TTML 文件、读取重建内存索引所需的元数据条目
//!
//! 查询与 `setup.rs` 中的 FTS DDL（列序、trigram tokenizer、同步触发器）必须保持同步，
//! 修改任何一边必须要同步另一边

use compact_str::CompactString;
use sea_orm::{
    ColumnTrait,
    ConnectionTrait,
    DatabaseBackend,
    DatabaseConnection,
    EntityTrait,
    FromQueryResult,
    QueryFilter,
    QuerySelect,
    Statement,
};
use serde_json::Value;

use crate::{
    core::{
        LyricId,
        db::entity,
        error::AppError,
        matcher::convert_tw2s,
        models::{
            LyricHit,
            LyricMatchField,
            SongEntry,
        },
    },
    utils::highlight::extract_lyric_context,
};

/// 按文件名读取数据库中的 TTML 文件，空内容视为缺失
///
/// 调用方需要从 GitHub 获取以兜底
pub async fn find_raw_ttml(conn: &DatabaseConnection, filename: &str) -> Option<String> {
    let row = entity::Entity::find()
        .filter(entity::Column::Filename.eq(filename))
        .one(conn)
        .await
        .ok()??;

    let ttml = row.raw_ttml;
    (!ttml.is_empty()).then_some(ttml)
}

/// 读取全部条目的元数据，用于重建内存索引
///
/// 只投影元数据列，`raw_ttml`、`lyric_text`、`bg_vocal_text` 不进行重建，避免短时大量内存开销
pub async fn fetch_song_entries(conn: &DatabaseConnection) -> Result<Vec<SongEntry>, AppError> {
    let rows = entity::Entity::find()
        .select_only()
        .columns([
            entity::Column::Id,
            entity::Column::Filename,
            entity::Column::Timestamp,
            entity::Column::TrackNames,
            entity::Column::ArtistNames,
            entity::Column::AlbumNames,
            entity::Column::NormalizedTrackNames,
            entity::Column::NormalizedArtistNames,
            entity::Column::NormalizedAlbumNames,
            entity::Column::NcmMusicIds,
            entity::Column::QqMusicIds,
            entity::Column::AppleMusicIds,
            entity::Column::SpotifyIds,
            entity::Column::Isrcs,
            entity::Column::AuthorIds,
            entity::Column::AuthorUsernames,
        ])
        .into_model::<MetadataRow>()
        .all(conn)
        .await
        .map_err(|e| AppError::InternalServerError(format!("Failed to load entries: {e}")))?;

    Ok(rows.into_iter().map(SongEntry::from).collect())
}

/// [`fetch_song_entries`] 的投影行形状，字段名与列名一致
#[derive(FromQueryResult)]
struct MetadataRow {
    id: i64,
    filename: String,
    timestamp: i64,
    track_names: Value,
    artist_names: Value,
    album_names: Value,
    normalized_track_names: Value,
    normalized_artist_names: Value,
    normalized_album_names: Value,
    ncm_music_ids: Value,
    qq_music_ids: Value,
    apple_music_ids: Value,
    spotify_ids: Value,
    isrcs: Value,
    author_ids: Value,
    author_usernames: Value,
}

impl From<MetadataRow> for SongEntry {
    fn from(row: MetadataRow) -> Self {
        Self {
            id: LyricId::from_u64_masked(row.id.cast_unsigned()),
            filename: CompactString::new(row.filename),
            timestamp: row.timestamp.cast_unsigned(),
            track_names: json_to_compact_box(row.track_names),
            artist_names: json_to_compact_box(row.artist_names),
            album_names: json_to_compact_box(row.album_names),
            normalized_track_names: json_to_compact_box(row.normalized_track_names),
            normalized_artist_names: json_to_compact_box(row.normalized_artist_names),
            normalized_album_names: json_to_compact_box(row.normalized_album_names),
            ncm_music_ids: json_to_compact_box(row.ncm_music_ids),
            qq_music_ids: json_to_compact_box(row.qq_music_ids),
            apple_music_ids: json_to_compact_box(row.apple_music_ids),
            spotify_ids: json_to_compact_box(row.spotify_ids),
            isrcs: json_to_compact_box(row.isrcs),
            author_ids: json_to_compact_box(row.author_ids),
            author_usernames: json_to_compact_box(row.author_usernames),
        }
    }
}

fn json_to_compact_box(value: Value) -> Box<[CompactString]> {
    serde_json::from_value::<Vec<String>>(value)
        .unwrap_or_default()
        .into_iter()
        .map(CompactString::from)
        .collect()
}

/// FTS5 全文检索歌词正文，返回按 bm25 相关度排序的命中
///
/// 关键词先做繁简转换与小写化，双引号转义后以短语形式传给 MATCH；
/// snippet 优先从主歌词提取，回退到背景人声
pub async fn search_fts(
    conn: &DatabaseConnection,
    keyword: &str,
    limit: u64,
) -> Result<Vec<LyricHit>, AppError> {
    if keyword.trim().is_empty() {
        return Ok(Vec::new());
    }

    let normalized = convert_tw2s(keyword).to_lowercase();
    let safe_keyword = normalized.replace('"', "\"\"");
    let match_expr = format!("\"{safe_keyword}\"");

    // `(0.0, 1.0, 0.25)` 对应 `(filename, lyric_text, bg_vocal_text)`
    let sql = r"
        SELECT
            rowid AS id,
            bm25(lyrics_fts, 0.0, 1.0, 0.25) AS rank,
            lyric_text,
            bg_vocal_text
        FROM lyrics_fts
        WHERE lyrics_fts MATCH $1
        ORDER BY rank ASC
        LIMIT $2;
    ";

    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Sqlite,
        sql,
        vec![match_expr.into(), limit.into()],
    );

    let rows = conn
        .query_all_raw(stmt)
        .await
        .map_err(|e| AppError::InternalServerError(format!("FTS Query Error: {e}")))?;

    let mut hits = Vec::with_capacity(rows.len());
    for row in rows {
        let id: i64 = match row.try_get("", "id") {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("Failed to read 'id' column from FTS result: {e}");
                continue;
            }
        };

        let rank: f64 = match row.try_get("", "rank") {
            Ok(rank) => rank,
            Err(e) => {
                tracing::warn!("Failed to read 'rank' column from FTS result: {e}");
                continue;
            }
        };

        let lyric_text: String = row.try_get("", "lyric_text").unwrap_or_default();
        let bg_vocal_text: String = row.try_get("", "bg_vocal_text").unwrap_or_default();

        let (field, snippet) = extract_lyric_context(&lyric_text, &normalized)
            .map(|s| (LyricMatchField::MainLyric, Some(s)))
            .or_else(|| {
                extract_lyric_context(&bg_vocal_text, &normalized)
                    .map(|s| (LyricMatchField::BackgroundVocal, Some(s)))
            })
            .unwrap_or((LyricMatchField::MainLyric, None));

        hits.push(LyricHit {
            id: LyricId::from_u64_masked(id.cast_unsigned()),
            rank,
            field,
            snippet,
        });
    }

    Ok(hits)
}

#[cfg(test)]
mod tests {
    use sea_orm::{
        EntityTrait,
        Set,
    };
    use serde_json::{
        Value,
        json,
    };

    use super::*;
    use crate::core::db::setup::init_db;

    fn seed_row(
        filename: &str,
        lyric_text: &str,
        bg_vocal_text: &str,
        raw_ttml: &str,
    ) -> entity::ActiveModel {
        let empty: Value = json!([]);
        entity::ActiveModel {
            id: Set(LyricId::from_filename(filename).get().cast_signed()),
            filename: Set(filename.to_string()),
            timestamp: Set(0),
            track_names: Set(empty.clone()),
            artist_names: Set(empty.clone()),
            album_names: Set(empty.clone()),
            normalized_track_names: Set(empty.clone()),
            normalized_artist_names: Set(empty.clone()),
            normalized_album_names: Set(empty.clone()),
            ncm_music_ids: Set(empty.clone()),
            qq_music_ids: Set(empty.clone()),
            apple_music_ids: Set(empty.clone()),
            spotify_ids: Set(empty.clone()),
            isrcs: Set(empty.clone()),
            author_ids: Set(empty.clone()),
            author_usernames: Set(empty),
            lyric_text: Set(lyric_text.to_string()),
            bg_vocal_text: Set(bg_vocal_text.to_string()),
            raw_ttml: Set(raw_ttml.to_string()),
        }
    }

    async fn seeded_conn() -> DatabaseConnection {
        let conn = init_db("sqlite::memory:").await.expect("init db");
        entity::Entity::insert(seed_row(
            "main_hit.ttml",
            "first line\nHello World Lyric\nlast line",
            "",
            "<tt/>",
        ))
        .exec(&conn)
        .await
        .expect("seed main row");

        entity::Entity::insert(seed_row(
            "bg_hit.ttml",
            "",
            "only in background vocal\nSunshine vocal",
            "<tt/>",
        ))
        .exec(&conn)
        .await
        .expect("seed bg row");

        entity::Entity::insert(seed_row("empty_text.ttml", "", "", "<tt/>"))
            .exec(&conn)
            .await
            .expect("seed empty row");

        conn
    }

    #[tokio::test]
    async fn fts_match_in_main_lyric_yields_marked_snippet() {
        let conn = seeded_conn().await;
        let hits = search_fts(&conn, "Hello", 10).await.expect("fts query");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].field, LyricMatchField::MainLyric);
        assert_eq!(
            hits[0].snippet.as_deref(),
            Some("first line\n<mark>Hello</mark> World Lyric\nlast line")
        );
        assert_eq!(hits[0].id, LyricId::from_filename("main_hit.ttml"));
    }

    #[tokio::test]
    async fn fts_match_falls_back_to_background_vocal() {
        let conn = seeded_conn().await;
        let hits = search_fts(&conn, "Sunshine", 10).await.expect("fts query");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].field, LyricMatchField::BackgroundVocal);
        assert_eq!(
            hits[0].snippet.as_deref(),
            Some("only in background vocal\n<mark>Sunshine</mark> vocal")
        );
    }

    #[tokio::test]
    async fn fts_blank_keyword_returns_empty() {
        let conn = seeded_conn().await;
        let hits = search_fts(&conn, "   ", 10).await.expect("fts query");
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn fts_keyword_with_quotes_does_not_error() {
        let conn = seeded_conn().await;
        let hits = search_fts(&conn, "He\"llo", 10).await.expect("fts query");
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn fts_respects_limit() {
        let conn = seeded_conn().await;
        let hits = search_fts(&conn, "line", 1).await.expect("fts query");
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn find_raw_ttml_hit_miss_and_empty() {
        let conn = seeded_conn().await;

        let found = find_raw_ttml(&conn, "main_hit.ttml").await;
        assert_eq!(found.as_deref(), Some("<tt/>"));

        let missing = find_raw_ttml(&conn, "nope.ttml").await;
        assert_eq!(missing, None);

        entity::Entity::insert(seed_row("blank.ttml", "x", "y", ""))
            .exec(&conn)
            .await
            .expect("seed blank row");
        let blank = find_raw_ttml(&conn, "blank.ttml").await;
        assert_eq!(blank, None);
    }

    #[tokio::test]
    async fn fetch_song_entries_round_trips_metadata() {
        let conn = init_db("sqlite::memory:").await.expect("init db");

        let mut row = seed_row("meta.ttml", "lyric", "bg", "<tt/>");
        row.track_names = Set(json!(["Test Song One"]));
        row.artist_names = Set(json!(["Artist Alpha"]));
        row.normalized_track_names = Set(json!(["test song one"]));
        row.spotify_ids = Set(json!(["sp1001"]));
        row.timestamp = Set(1_600_000_000);
        entity::Entity::insert(row)
            .exec(&conn)
            .await
            .expect("seed row");

        let entries = fetch_song_entries(&conn).await.expect("fetch entries");
        assert_eq!(entries.len(), 1);

        let entry = &entries[0];
        assert_eq!(entry.id, LyricId::from_filename("meta.ttml"));
        assert_eq!(entry.filename.as_str(), "meta.ttml");
        assert_eq!(entry.timestamp, 1_600_000_000);
        assert_eq!(entry.track_names[0].as_str(), "Test Song One");
        assert_eq!(entry.artist_names[0].as_str(), "Artist Alpha");
        assert_eq!(entry.normalized_track_names[0].as_str(), "test song one");
        assert_eq!(entry.spotify_ids[0].as_str(), "sp1001");
    }

    #[tokio::test]
    async fn fetch_song_entries_empty_table() {
        let conn = init_db("sqlite::memory:").await.expect("init db");
        let entries = fetch_song_entries(&conn).await.expect("fetch entries");
        assert!(entries.is_empty());
    }
}
