//! 请求统计的 SQLite 热缓冲
//!
//! 独立于歌词主库的文件。新请求先落在这里，崩溃最多丢掉写入任务里还没刷盘的约 1 秒数据。
//! 蓝绿切换时新旧两个实例会同时写同一个文件，靠 WAL 与 `busy_timeout` 跨进程串行化写锁
//!
//! 表结构与 [`insert_batch`] 的列序互为同一契约的两半，改任一侧必须同步另一侧

use std::{
    path::Path,
    time::Duration,
};

use sea_orm::{
    ConnectOptions,
    ConnectionTrait,
    Database,
    DatabaseBackend,
    DatabaseConnection,
    DbErr,
    Statement,
    TransactionTrait,
    sqlx::sqlite::{
        SqliteJournalMode,
        SqliteSynchronous,
    },
};

use super::{
    annotation::MatchKind,
    record::RequestRecord,
};

/// 表结构版本，写进 `PRAGMA user_version`，改表结构时递增
const SCHEMA_VERSION: u32 = 1;

/// 另一个实例持有写锁时的最长等待。写入任务每秒只提交一次事务，
/// 正常竞争远到不了这个值
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const CREATE_REQUESTS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS requests (
    ts INTEGER NOT NULL,
    method TEXT NOT NULL,
    prefix TEXT,
    route TEXT,
    raw_path TEXT,
    params TEXT,
    status INTEGER NOT NULL,
    latency_us INTEGER NOT NULL,
    resp_bytes INTEGER,
    conditional INTEGER NOT NULL,
    user_agent TEXT,
    origin TEXT,
    referer TEXT,
    client_id INTEGER,
    instance TEXT NOT NULL,
    hit_count INTEGER,
    hit_id INTEGER,
    match_kind TEXT,
    norm_query TEXT
);
";

/// 按时间段导出和删除（每日转换）都走 `ts` 范围
const CREATE_TS_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS requests_ts ON requests (ts);";

const INSERT_SQL: &str = r"
INSERT INTO requests (
    ts, method, prefix, route, raw_path, params, status, latency_us, resp_bytes, conditional,
    user_agent, origin, referer, client_id, instance, hit_count, hit_id, match_kind, norm_query
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
";

/// 打开（必要时创建）缓冲库并建表
pub async fn open(path: &Path) -> Result<DatabaseConnection, DbErr> {
    let path = path.to_owned();

    // URL 只用来让 sea-orm 选中 SQLite 后端，真正的文件路径在下面覆盖，
    // 避免 Windows 路径拼进 URL 时的转义问题
    let mut options = ConnectOptions::new("sqlite://analytics.db");
    options
        .max_connections(1)
        .sqlx_logging(false)
        .map_sqlx_sqlite_opts(move |opts| {
            opts.filename(&path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal)
                .busy_timeout(BUSY_TIMEOUT)
        });

    let db = Database::connect(options).await?;
    db.execute_unprepared(CREATE_REQUESTS_TABLE_SQL).await?;
    db.execute_unprepared(CREATE_TS_INDEX_SQL).await?;
    db.execute_unprepared(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
        .await?;

    Ok(db)
}

/// 在一个事务里写入一批记录
pub async fn insert_batch(db: &DatabaseConnection, records: &[RequestRecord]) -> Result<(), DbErr> {
    let txn = db.begin().await?;

    for record in records {
        let values = vec![
            record.ts.into(),
            record.method.clone().into(),
            record.prefix.into(),
            record.route.clone().into(),
            record.raw_path.clone().into(),
            record.params.clone().into(),
            i32::from(record.status).into(),
            record.latency_us.into(),
            record.resp_bytes.into(),
            record.conditional.into(),
            record.user_agent.clone().into(),
            record.origin.clone().into(),
            record.referer.clone().into(),
            record.client_id.into(),
            record.instance.as_ref().into(),
            record.hit_count.into(),
            record.hit_id.into(),
            record.match_kind.map(MatchKind::as_str).into(),
            record.norm_query.clone().into(),
        ];

        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            INSERT_SQL,
            values,
        ))
        .await?;
    }

    txn.commit().await
}
