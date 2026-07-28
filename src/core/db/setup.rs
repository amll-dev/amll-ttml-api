use std::{
    fs,
    path::Path,
};

use sea_orm::{
    ConnectionTrait,
    Database,
    DatabaseConnection,
    DbErr,
};
use tracing::info;

const CREATE_LYRICS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS lyrics (
    id BIGINT PRIMARY KEY,
    filename TEXT NOT NULL UNIQUE,
    timestamp BIGINT NOT NULL,
    track_names JSON NOT NULL,
    artist_names JSON NOT NULL,
    album_names JSON NOT NULL,
    normalized_track_names JSON NOT NULL,
    normalized_artist_names JSON NOT NULL,
    normalized_album_names JSON NOT NULL,
    ncm_music_ids JSON NOT NULL,
    qq_music_ids JSON NOT NULL,
    apple_music_ids JSON NOT NULL,
    spotify_ids JSON NOT NULL,
    isrcs JSON NOT NULL,
    author_ids JSON NOT NULL,
    author_usernames JSON NOT NULL,
    lyric_text TEXT NOT NULL,
    bg_vocal_text TEXT NOT NULL,
    raw_ttml TEXT NOT NULL
);
";

const CREATE_META_INFO_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS meta_info (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

const CREATE_LYRICS_FTS_TABLE_SQL: &str = r"
CREATE VIRTUAL TABLE IF NOT EXISTS lyrics_fts USING fts5(
    filename UNINDEXED,
    lyric_text,
    bg_vocal_text,
    tokenize = 'trigram'
);
";

const FTS_TRIGGERS: [&str; 3] = [
    r"
    CREATE TRIGGER IF NOT EXISTS lyrics_ai AFTER INSERT ON lyrics BEGIN
        INSERT INTO lyrics_fts(rowid, filename, lyric_text, bg_vocal_text)
        VALUES (new.id, new.filename, new.lyric_text, new.bg_vocal_text);
    END;
    ",
    r"
    CREATE TRIGGER IF NOT EXISTS lyrics_ad AFTER DELETE ON lyrics BEGIN
        INSERT INTO lyrics_fts(lyrics_fts, rowid, filename, lyric_text, bg_vocal_text)
        VALUES('delete', old.id, old.filename, old.lyric_text, old.bg_vocal_text);
    END;
    ",
    r"
    CREATE TRIGGER IF NOT EXISTS lyrics_au AFTER UPDATE ON lyrics BEGIN
        INSERT INTO lyrics_fts(lyrics_fts, rowid, filename, lyric_text, bg_vocal_text)
        VALUES('delete', old.id, old.filename, old.lyric_text, old.bg_vocal_text);
        INSERT INTO lyrics_fts(rowid, filename, lyric_text, bg_vocal_text)
        VALUES (new.id, new.filename, new.lyric_text, new.bg_vocal_text);
    END;
    ",
];

pub async fn init_db(db_url: &str) -> Result<DatabaseConnection, DbErr> {
    if let Some(file_path) = db_url.strip_prefix("sqlite://")
        && let Some(parent) = Path::new(file_path).parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
        && let Err(e) = fs::create_dir_all(parent)
    {
        tracing::error!("Failed to create database directory {parent:?}: {e}");
    }

    let db = Database::connect(db_url).await?;

    db.execute_unprepared("PRAGMA journal_mode=WAL;").await?;
    db.execute_unprepared("PRAGMA synchronous=NORMAL;").await?;
    db.execute_unprepared(CREATE_LYRICS_TABLE_SQL).await?;
    db.execute_unprepared(CREATE_META_INFO_TABLE_SQL).await?;

    let fts_res = db.execute_unprepared(CREATE_LYRICS_FTS_TABLE_SQL).await;

    if let Err(e) = fts_res {
        tracing::warn!("Failed to create FTS5 table (might not be supported by sqlite build): {e}");
    } else {
        for t in FTS_TRIGGERS {
            if let Err(e) = db.execute_unprepared(t).await {
                tracing::warn!("Failed to create FTS trigger: {e}");
            }
        }
    }

    info!("Database setup completed successfully");
    Ok(db)
}
